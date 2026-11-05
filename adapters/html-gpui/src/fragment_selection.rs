//! GPUI focus and gestures backed by the engine's logical selection and geometry.
use html_pipeline::engine::{export_queries, geometry_queries, text_paint};
#[cfg(test)]
use html_pipeline::engine::search_queries;
use html_pipeline::engine::TextExportFormat;

use gpui::{FocusHandle, Pixels, Point};
use html::layout::RenderView;
use html::selection::{
    SelectionController, SelectionInput, SelectionLayout, SelectionMode, SelectionOutcome,
};
use html_render_core::RegionProjection;
use html::render::LinePlacement;
use kurbo::{Point as DocumentPoint, Rect};

const WORD_SELECTION_DRAG_DISTANCE: f64 = 4.0;

pub(crate) struct FragmentSelection {
    pub focus: FocusHandle,
    logical: SelectionController,
    interaction: SelectionOutcome,
}

fn document_point(point: Point<Pixels>) -> DocumentPoint {
    DocumentPoint::new(f64::from(f32::from(point.x)), f64::from(f32::from(point.y)))
}

impl FragmentSelection {
    pub fn new(focus: FocusHandle) -> Self {
        Self {
            focus,
            logical: SelectionController::default(),
            interaction: SelectionOutcome::default(),
        }
    }

    pub fn clear(&mut self) {
        self.logical = SelectionController::default();
        self.interaction = SelectionOutcome::default();
    }
    pub fn is_empty(&self, view: RenderView<'_>) -> bool {
        self.logical.selection(view).is_none()
    }
    pub fn dragging(&self) -> bool {
        self.interaction.dragging
    }
    pub fn finish(&mut self, view: RenderView<'_>) {
        self.interact(view, SelectionInput::Finish);
    }

    pub fn selected_text(&self, view: RenderView<'_>) -> String {
        self.logical
            .selection(view)
            .map(|selection| selection.range())
            .and_then(|range| {
                export_queries::render_export_view_text(
                    view.export(),
                    range,
                    TextExportFormat::PlainWithParagraphBreaks,
                )
                .plain
            })
            .unwrap_or_default()
    }

    pub fn rects(&self, view: RenderView<'_>) -> Vec<Rect> {
        let Some(range) = self
            .logical
            .selection(view)
            .map(|selection| selection.range())
        else {
            return Vec::new();
        };
        view.text()
            .lines()
            .iter()
            .flat_map(|line| {
                let clip = text_paint::line_overflow_clip(view.text(), line.id());
                geometry_queries::render_geometry_view_line_selection_rects(
                    geometry_queries::render_view_geometry(view),
                    line.id(),
                    range.clone(),
                )
                .map(move |rect| clip.map_or(rect, |clip| clip.apply_to(rect)))
                .filter(|rect| rect.width() > 0.0 && rect.height() > 0.0)
            })
            .collect()
    }

    fn interact(&mut self, view: RenderView<'_>, input: SelectionInput) -> bool {
        let placements = view.text().lines().iter().map(|line| LinePlacement {
            line: line.id(),
            origin: line.point(),
        });
        let projection = RegionProjection {
            source: Rect::new(
                -f64::MAX / 2.0,
                -f64::MAX / 2.0,
                f64::MAX / 2.0,
                f64::MAX / 2.0,
            ),
            destination: DocumentPoint::new(-f64::MAX / 2.0, -f64::MAX / 2.0),
        };
        self.interaction = self.logical.interact(
            SelectionLayout {
                view,
                placements,
                projection: &projection,
            },
            input,
        );
        self.interaction.handled
    }

    pub fn select_all(&mut self, view: RenderView<'_>) {
        self.interact(
            view,
            SelectionInput::SetRange {
                range: 0..view.text().glyph_count() as u32,
                mode: SelectionMode::Plain,
            },
        );
    }

    pub fn begin(&mut self, view: RenderView<'_>, position: Point<Pixels>, clicks: usize) -> bool {
        self.clear();
        self.interact(
            view,
            SelectionInput::PointerDown {
                position: document_point(position),
                mode: SelectionMode::Plain,
                select_text: true,
                clicks,
            },
        )
    }

    pub fn extend(&mut self, view: RenderView<'_>, position: Point<Pixels>) {
        self.interact(
            view,
            SelectionInput::PointerMove {
                position: document_point(position),
                mode: SelectionMode::Plain,
                drag_distance: WORD_SELECTION_DRAG_DISTANCE,
            },
        );
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::GpuiTextCache;
    use gpui::{EmptyView, TestAppContext, point, px};
    use html_pipeline::{PipelineChanges, PipelineSession};
    use crate::fragment::load_fragment;

    /// Lay out `source` at 300px, replacing the previous fragment's source.
    fn render<'a>(
        document: &'a mut Option<PipelineSession>,
        shaper: &mut GpuiTextCache,
        source: &str,
    ) -> RenderView<'a> {
        let styles =
            vec!["html,body,p { margin:0; padding:0; font-size:20px; line-height:30px; }".to_owned()];
        match document {
            Some(document) => {
                document
                    .attach(shaper).apply_changes(PipelineChanges {
                        source: Some(source.to_owned()), user_styles: Some(styles),
                        ..PipelineChanges::default()
                    })
                    .unwrap();
            }
            None => {
                *document = Some(load_fragment(source.to_owned(), 300.0, styles, shaper).unwrap());
            }
        }
        document.as_ref().unwrap().render_view().unwrap()
    }

    #[gpui::test]
    fn double_click_drag_extends_and_shrinks_around_the_initial_word(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, cx| {
                let mut renderer = None;
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let view = render(&mut renderer, &mut shaper, "<p>alpha beta gamma</p>");
                let geometry = geometry_queries::render_view_geometry(view);
                let word_point = |word: &str| {
                    let range = search_queries::render_text_view_find_matches(
                        view.text(),
                        word,
                        Default::default(),
                    )[0]
                    .clone();
                    let center = geometry_queries::selection_bounds_lines(
                        geometry,
                        range,
                        view.text()
                            .lines()
                            .iter()
                            .map(|line| LinePlacement {
                                line: line.id(),
                                origin: line.point(),
                            })
                            .into_iter()
                            .map(|placement| (placement.line, placement.origin)),
                    )
                    .unwrap()
                    .center();
                    point(px(center.x as f32), px(center.y as f32))
                };
                let mut selection = FragmentSelection::new(cx.focus_handle());
                assert!(selection.begin(view, word_point("beta"), 2));
                assert!(selection.dragging());
                assert_eq!(selection.selected_text(view), "beta");
                for (word, expected) in [
                    ("gamma", "beta gamma"),
                    ("beta", "beta"),
                    ("alpha", "alpha beta"),
                ] {
                    selection.extend(view, word_point(word));
                    assert_eq!(selection.selected_text(view), expected);
                }
                selection.finish(view);
                assert!(!selection.dragging());
                selection.extend(view, word_point("gamma"));
                assert_eq!(selection.selected_text(view), "alpha beta");
            })
            .unwrap();
    }

    #[gpui::test]
    fn description_selection_can_start_in_paragraph_padding(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, cx| {
                let mut renderer = None;
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
    fn selection_keeps_words_and_graphemes_whole_and_respects_rtl_carets(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, cx| {
                let mut renderer = None;
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let view = render(&mut renderer, &mut shaper, "<p>A👩‍👧e\u{301}Z</p>");
                let emoji = geometry_queries::selection_bounds_lines(
                    geometry_queries::render_view_geometry(view),
                    1..4,
                    view.text()
                        .lines()
                        .iter()
                        .map(|line| LinePlacement {
                            line: line.id(),
                            origin: line.point(),
                        })
                        .into_iter()
                        .map(|placement| (placement.line, placement.origin)),
                )
                .unwrap();
                let accent = geometry_queries::selection_bounds_lines(
                    geometry_queries::render_view_geometry(view),
                    4..6,
                    view.text()
                        .lines()
                        .iter()
                        .map(|line| LinePlacement {
                            line: line.id(),
                            origin: line.point(),
                        })
                        .into_iter()
                        .map(|placement| (placement.line, placement.origin)),
                )
                .unwrap();
                let start = point(px(emoji.x0 as f32 + 0.1), px(emoji.center().y as f32));
                let end = point(px(accent.x1 as f32 - 0.1), px(accent.center().y as f32));
                let mut selection = FragmentSelection::new(cx.focus_handle());
                assert!(selection.begin(view, start, 1));
                selection.extend(view, end);
                assert_eq!(selection.selected_text(view), "👩‍👧e\u{301}Z");
                assert!(selection.begin(view, end, 1));
                selection.extend(view, start);
                assert_eq!(selection.selected_text(view), "👩‍👧e\u{301}Z");

                let view = render(&mut renderer, &mut shaper, "<p dir='rtl'>אב</p>");
                let bounds = geometry_queries::selection_bounds_lines(
                    geometry_queries::render_view_geometry(view),
                    0..2,
                    view.text()
                        .lines()
                        .iter()
                        .map(|line| LinePlacement {
                            line: line.id(),
                            origin: line.point(),
                        })
                        .into_iter()
                        .map(|placement| (placement.line, placement.origin)),
                )
                .unwrap();
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
            let mut renderer = None;
            let mut shaper = GpuiTextCache::new(window.text_system().clone());
            let view = render(&mut renderer, &mut shaper, "<div style='height:30px;overflow:hidden'><p>Visible</p><p>Clipped</p></div><p>Last <em>paragraph</em></p>");
            let mut selection = FragmentSelection::new(cx.focus_handle());
            selection.select_all(view);
            // The hidden line must never be chosen when dragging through its original position.
            let hidden = search_queries::render_text_view_find_matches(view.text(), "Clipped", Default::default())[0].clone();
            selection.interact(view, SelectionInput::SetRange { range: hidden.clone(), mode: SelectionMode::Plain });
            assert!(selection.rects(view).is_empty(), "clipped text must have no selection ink");
            assert!(selection.begin(view, point(px(10.0), px(45.0)), 1));
            selection.finish(view);
            assert!(!hidden.contains(&selection.interaction.click.unwrap()));
            let last = search_queries::render_text_view_find_matches(view.text(), "Last", Default::default())[0].clone();
            let rect = geometry_queries::selection_bounds_lines(geometry_queries::render_view_geometry(view),last,view.text().lines().iter().map(|line| LinePlacement { line: line.id(), origin: line.point() }).into_iter().map(|placement| (placement.line, placement.origin))).unwrap();
            assert!(selection.begin(view, point(px(rect.center().x as f32), px(rect.center().y as f32)), 3));
            assert_eq!(selection.selected_text(view), "Last paragraph");
            let view = render(&mut renderer, &mut shaper, "<p>office</p>");
            let ligature = geometry_queries::selection_bounds_lines(geometry_queries::render_view_geometry(view),1..3,view.text().lines().iter().map(|line| LinePlacement { line: line.id(), origin: line.point() }).into_iter().map(|placement| (placement.line, placement.origin))).unwrap();
            let point = point(px(ligature.center().x as f32), px(ligature.center().y as f32));
            assert!(selection.begin(view, point, 2));
            assert_eq!(selection.selected_text(view), "office");

        }).unwrap();
    }
}
