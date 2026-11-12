//! GPUI focus and gestures backed by the engine's logical selection and geometry.
use crate::paint::{GpuiCommandPainter, GpuiScene};
use gpui::{Pixels, Point};
use html::layout::RenderView;
use html::selection::{
    SelectionController, SelectionExport, SelectionInput, SelectionKind, SelectionLayout,
    SelectionMode, SelectionOutcome, SelectionPaintScratch, SelectionPaintStyle};
use html_render_core::RegionProjection;
use html::render::LinePlacement;
use kurbo::{Point as DocumentPoint, Rect};
use peniko::Color;

const WORD_SELECTION_DRAG_DISTANCE: f64 = 4.0;

#[derive(Default)]
pub(crate) struct FragmentSelection {
    logical: SelectionController,
    interaction: SelectionOutcome,
    paint_scratch: SelectionPaintScratch,
    scene: Option<GpuiScene>,
    paint_style: Option<(Color, Option<u32>)>,
    pressed_link: Option<(DocumentPoint, String)>,
    /// Retain the press through release, including clicks consumed by dismissal.
    pressed_selectable: Option<bool>,
}

fn document_point(point: Point<Pixels>) -> DocumentPoint {
    DocumentPoint::new(f64::from(f32::from(point.x)), f64::from(f32::from(point.y)))
}

fn fragment_projection() -> RegionProjection {
    RegionProjection {
        source: Rect::new(
            -f64::MAX / 2.0,
            -f64::MAX / 2.0,
            f64::MAX / 2.0,
            f64::MAX / 2.0,
        ),
        destination: DocumentPoint::new(-f64::MAX / 2.0, -f64::MAX / 2.0),
    }
}

impl FragmentSelection {
    pub fn clear(&mut self) {
        self.logical = SelectionController::default();
        self.interaction = SelectionOutcome::default();
        self.paint_style = None;
        self.pressed_link = None;
        self.pressed_selectable = None;
    }
    pub fn clear_selection(&mut self, view: RenderView<'_>) {
        self.pressed_link = None;
        self.pressed_selectable = None;
        self.interact(view, SelectionInput::Clear(SelectionKind::Text));
        self.interact(view, SelectionInput::Clear(SelectionKind::Table));
    }

    pub fn is_empty(&self, view: RenderView<'_>) -> bool {
        self.logical.selection(view).is_none()
    }
    pub fn dragging(&self) -> bool {
        self.interaction.dragging
    }
    pub fn pointer_active(&self) -> bool {
        self.pressed_selectable.is_some() || self.dragging() || self.pressed_link.is_some()
    }
    pub fn finish(&mut self, view: RenderView<'_>) {
        self.pressed_link = None;
        self.pressed_selectable = None;
        self.interact(view, SelectionInput::Finish);
    }

    pub fn pointer_down(
        &mut self,
        view: RenderView<'_>,
        position: Point<Pixels>,
        clicks: usize,
        selectable: bool,
    ) -> bool {
        self.pressed_link = None;
        let handled = if selectable {
            self.begin(view, position, clicks)
        } else {
            self.pointer_move(view, Some(position));
            false
        };
        if clicks == 1 && (!selectable || self.dragging()) {
            self.pressed_link = self
                .logical
                .hovered_link(view)
                .map(|link| (document_point(position), link.href.to_owned()));
        }
        let handled = handled || self.pressed_link.is_some();
        self.pressed_selectable = handled.then_some(selectable);
        handled
    }

    pub fn pointer_up(
        &mut self,
        view: RenderView<'_>,
        position: Point<Pixels>,
        hovered: bool,
    ) -> Option<String> {
        let pressed = self.pressed_link.take();
        let selectable = self.pressed_selectable.take().unwrap_or(false);
        let position = document_point(position);
        self.interact(
            view,
            SelectionInput::PointerUp {
                position,
                mode: SelectionMode::Plain,
                drag_distance: WORD_SELECTION_DRAG_DISTANCE,
            },
        );
        if !hovered {
            self.pointer_move(view, None);
            return None;
        }
        // The shared controller decides whether this was an activating click,
        // a dismissal, or a drag. Nonselectable fragments only track links.
        if selectable && self.interaction.click.is_none() {
            return None;
        }
        let (start, href) = pressed?;
        (start.distance_squared(position) < WORD_SELECTION_DRAG_DISTANCE.powi(2)
            && self
                .logical
                .hovered_link(view)
                .is_some_and(|link| link.href == href))
        .then_some(href)
    }

    pub fn selected_text(&self, view: RenderView<'_>) -> String {
        self.logical
            .export(
                SelectionLayout {
                    view,
                    placements: std::iter::empty(),
                    projection: &fragment_projection(),
                },
                SelectionExport::PlainText,
            )
            .unwrap_or_default()
    }

    pub fn paint(
        &mut self,
        view: RenderView<'_>,
        color: Color,
        hover_foreground: Option<u32>,
    ) -> Option<&GpuiScene> {
        if self.is_empty(view) && !self.link_hovered() {
            return None;
        }
        if self.paint_style != Some((color, hover_foreground)) {
            let mut painter = GpuiCommandPainter::commands_only();
            if let Some(previous) = self.scene.take() {
                painter = painter.reusing(previous);
            }
            self.logical.paint(
                SelectionLayout {
                    view,
                    placements: view.text().lines().iter().map(|line| LinePlacement {
                        line: line.id(),
                        origin: line.point(),
                    }),
                    projection: &fragment_projection(),
                },
                SelectionPaintStyle {
                    selection: color,
                    hover_foreground,
                },
                &mut painter,
                &mut self.paint_scratch,
            );
            self.scene = Some(painter.finish());
            self.paint_style = Some((color, hover_foreground));
        }
        self.scene.as_ref()
    }

    pub fn link_hovered(&self) -> bool {
        self.logical.link_hovered()
    }

    pub fn pointer_move(&mut self, view: RenderView<'_>, position: Option<Point<Pixels>>) -> bool {
        if self.pressed_link.as_ref().is_some_and(|(start, _)| {
            position.is_none_or(|position| {
                start.distance_squared(document_point(position))
                    >= WORD_SELECTION_DRAG_DISTANCE.powi(2)
            })
        }) {
            self.pressed_link = None;
        }
        match position {
            Some(position) => self.interact(
                view,
                SelectionInput::PointerMove {
                    position: document_point(position),
                    mode: SelectionMode::Plain,
                    drag_distance: WORD_SELECTION_DRAG_DISTANCE,
                },
            ),
            None => self.interact(view, SelectionInput::PointerLeave),
        }
    }

    fn interact(&mut self, view: RenderView<'_>, input: SelectionInput) -> bool {
        let hover = match &input {
            SelectionInput::PointerMove { position, .. } if !self.dragging() => Some(*position),
            _ => None,
        };
        let nearby = hover.into_iter().flat_map(move |point| {
            (view.geometry()).lines_at_y(point.y)
            .filter_map(move |id| view.text().line(id))
            .map(|line| LinePlacement {
                line: line.id(),
                origin: line.point(),
            })
        });
        let all = hover
            .is_none()
            .then_some(view)
            .into_iter()
            .flat_map(|view| {
                view.text().lines().iter().map(|line| LinePlacement {
                    line: line.id(),
                    origin: line.point(),
                })
            });
        let placements = nearby.chain(all);
        let projection = fragment_projection();
        self.interaction = self.logical.interact(
            SelectionLayout {
                view,
                placements,
                projection: &projection,
            },
            input,
        );
        if self.interaction.changed {
            self.paint_style = None;
        }
        self.interaction.handled || self.interaction.changed
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
        self.pressed_link = None;
        let handled = self.interact(
            view,
            SelectionInput::PointerDown {
                position: document_point(position),
                mode: SelectionMode::Plain,
                select_text: true,
                click_action: false,
                clicks,
            },
        );
        self.pressed_selectable = handled.then_some(true);
        handled
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
            .update(cx, |_, window, _| {
                let mut renderer = None;
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let view = render(&mut renderer, &mut shaper, "<p>alpha beta gamma</p>");
                let geometry = view.geometry();
                let word_point = |word: &str| {
                    let range = (view.text()).find_matches(word, Default::default())[0]
                    .clone();
                    let center = geometry.selection_bounds_lines(range, view.text()
                            .lines()
                            .iter()
                            .map(|line| LinePlacement {
                                line: line.id(),
                                origin: line.point(),
                            })
                            .into_iter()
                            .map(|placement| (placement.line, placement.origin)))
                    .unwrap()
                    .center();
                    point(px(center.x as f32), px(center.y as f32))
                };
                let mut selection = FragmentSelection::default();
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
            .update(cx, |_, window, _| {
                let mut renderer = None;
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let view = render(&mut renderer, &mut shaper, "<p>Hi</p>");
                let mut selection = FragmentSelection::default();
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
            .update(cx, |_, window, _| {
                let mut renderer = None;
                let mut shaper = GpuiTextCache::new(window.text_system().clone());
                let view = render(&mut renderer, &mut shaper, "<p>A👩‍👧e\u{301}Z</p>");
                let emoji = (view.geometry()).selection_bounds_lines(1..4, view.text()
                        .lines()
                        .iter()
                        .map(|line| LinePlacement {
                            line: line.id(),
                            origin: line.point(),
                        })
                        .into_iter()
                        .map(|placement| (placement.line, placement.origin)))
                .unwrap();
                let accent = (view.geometry()).selection_bounds_lines(4..6, view.text()
                        .lines()
                        .iter()
                        .map(|line| LinePlacement {
                            line: line.id(),
                            origin: line.point(),
                        })
                        .into_iter()
                        .map(|placement| (placement.line, placement.origin)))
                .unwrap();
                let start = point(px(emoji.x0 as f32 + 0.1), px(emoji.center().y as f32));
                let end = point(px(accent.x1 as f32 - 0.1), px(accent.center().y as f32));
                let mut selection = FragmentSelection::default();
                assert!(selection.begin(view, start, 1));
                selection.extend(view, end);
                assert_eq!(selection.selected_text(view), "👩‍👧e\u{301}Z");
                assert!(selection.begin(view, end, 1));
                selection.extend(view, start);
                assert_eq!(selection.selected_text(view), "👩‍👧e\u{301}Z");

                let view = render(&mut renderer, &mut shaper, "<p dir='rtl'>אב</p>");
                let bounds = (view.geometry()).selection_bounds_lines(0..2, view.text()
                        .lines()
                        .iter()
                        .map(|line| LinePlacement {
                            line: line.id(),
                            origin: line.point(),
                        })
                        .into_iter()
                        .map(|placement| (placement.line, placement.origin)))
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
        window.update(cx, |_, window, _| {
            let mut renderer = None;
            let mut shaper = GpuiTextCache::new(window.text_system().clone());
            let view = render(&mut renderer, &mut shaper, "<div style='height:30px;overflow:hidden'><p>Visible</p><p>Clipped</p></div><p>Last <em>paragraph</em></p>");
            let mut selection = FragmentSelection::default();
            selection.select_all(view);
            // The hidden line must never be chosen when dragging through its original position.
            let hidden = (view.text()).find_matches("Clipped", Default::default())[0].clone();
            selection.interact(view, SelectionInput::SetRange { range: hidden.clone(), mode: SelectionMode::Plain });
            assert!(selection.paint(view, Color::rgba8(0, 0, 0, 255), None).is_none_or(|scene| scene.len() == 0), "clipped text must have no selection ink");
            assert!(selection.begin(view, point(px(10.0), px(45.0)), 1));
            selection.finish(view);
            assert!(!hidden.contains(&selection.interaction.click.unwrap()));
            let last = (view.text()).find_matches("Last", Default::default())[0].clone();
            let rect = (view.geometry()).selection_bounds_lines(last, view.text().lines().iter().map(|line| LinePlacement { line: line.id(), origin: line.point() }).into_iter().map(|placement| (placement.line, placement.origin))).unwrap();
            assert!(selection.begin(view, point(px(rect.center().x as f32), px(rect.center().y as f32)), 3));
            assert_eq!(selection.selected_text(view), "Last paragraph");
            let view = render(&mut renderer, &mut shaper, "<p>office</p>");
            let ligature = (view.geometry()).selection_bounds_lines(1..3, view.text().lines().iter().map(|line| LinePlacement { line: line.id(), origin: line.point() }).into_iter().map(|placement| (placement.line, placement.origin))).unwrap();
            let point = point(px(ligature.center().x as f32), px(ligature.center().y as f32));
            assert!(selection.begin(view, point, 2));
            assert_eq!(selection.selected_text(view), "office");

        }).unwrap();
    }
}
