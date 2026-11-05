//! Continuous HTML fragments embedded in ordinary GPUI layouts.


#[cfg(test)]
use html_pipeline::engine::geometry_queries;
#[cfg(test)]
use html_pipeline::engine::text_paint;
#[cfg(test)]
use html_engine_model::layout_state;
use std::panic::Location;
use std::sync::Arc;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, ContentMask, CursorStyle, Element, ElementId,
    Entity, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement,
    KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    SharedString, Size, Style, Window, fill, point, px, relative, size,
};
use html::layout::RenderView;
use html::render::SceneImagePolicy;
use html_pipeline::{PipelineChanges, PipelineError, PipelineInputs, PipelineSession, StyleEnvironment, MarkupSyntax, MediaEnvironment};
use html_pipeline::engine::{LayoutConstraints, LayoutConstraintError};
use html::text::GlyphShaper;
use html_resources::resources::provider::NoResources;
use html_render_core::{self as render_core, RenderScene, SceneOptions};

use crate::fragment_selection::FragmentSelection;
use crate::paint::{GpuiCommandPainter, GpuiScene};
use crate::text::GpuiTextCache;

/// Natural-height HTML, with shaping and painting shared with the reader.
/// The enclosing layout owns scrolling; selection can be enabled independently
/// of a reader session.
pub struct HtmlFragmentElement {
    id: ElementId,
    source: SharedString,
    styles: Vec<String>,
    text_color: Option<Hsla>,
    selection_color: Option<Hsla>,
    state: Option<Entity<FragmentState>>,
}

impl HtmlFragmentElement {
    pub fn new(id: impl Into<ElementId>, source: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            styles: Vec::new(),
            text_color: None,
            selection_color: None,
            state: None,
        }
    }

    /// Supply CSS for the fragment. HTML and CSS own typography and colors;
    /// surrounding GPUI text styles are not inherited.
    pub fn stylesheet(mut self, css: impl Into<String>) -> Self {
        self.styles.push(css.into());
        self
    }

    /// Paint every text run in this color, including authored CSS colors.
    /// Typography, layout, and non-text colors remain controlled by HTML/CSS.
    pub fn text_color(mut self, color: Hsla) -> Self {
        self.text_color = Some(color);
        self
    }

    /// Enable mouse selection and clipboard shortcuts, using this highlight.
    pub fn selectable(mut self, color: Hsla) -> Self {
        self.selection_color = Some(color);
        self
    }

    /// Justified paragraphs select Knuth–Plass in the HTML engine, including
    /// under its web-compatible composition policy.
    pub fn justify(self) -> Self {
        self.stylesheet("body, p, li, div, blockquote { text-align: justify !important; }")
    }
}

#[derive(Clone, PartialEq)]
struct FragmentLayoutKey {
    source: SharedString,
    width: f32,
    styles: Arc<[String]>,
}

#[derive(Clone, PartialEq)]
struct FragmentKey {
    layout: FragmentLayoutKey,
    text_color: Option<Hsla>,
}

struct FragmentDisplay {
    key: FragmentKey,
    scene: GpuiScene,
    height: f32,
}

struct FragmentState {
    document: Option<PipelineSession>,
    shaper: GpuiTextCache,
    display: Option<FragmentDisplay>,
    selection: Option<FragmentSelection>,
    selection_key: Option<FragmentLayoutKey>,
}

impl FragmentState {
    fn new(window: &Window, cx: &App) -> Self {
        Self {
            document: None,
            shaper: GpuiTextCache::for_window(window, cx),
            display: None,
            selection: None,
            selection_key: None,
        }
    }

    fn layout(&mut self, key: FragmentKey) -> Result<Size<Pixels>, String> {
        let result = self.try_layout(key);
        if result.is_err() {
            self.display = None;
            self.selection_key = None;
            if let Some(selection) = &mut self.selection {
                selection.clear();
            }
        }
        result
    }

    fn try_layout(&mut self, key: FragmentKey) -> Result<Size<Pixels>, String> {
        let width = key.layout.width;
        if let Some(display) = &self.display
            && display.key == key
        {
            return Ok(size(px(width), px(display.height)));
        }
        let previous = self.display.as_ref().map(|display| &display.key.layout);
        let document = match &mut self.document {
            Some(document) => {
                if previous != Some(&key.layout) {
                    let layout = LayoutConstraints::new(f64::from(width), 16.0)
                        .map_err(|error| error.to_string())?;
                    let media = MediaEnvironment::screen(f64::from(width), None)
                        .ok_or("invalid fragment width")?;
                    let changes = PipelineChanges {
                        layout: Some(layout),
                        style_environment: Some(StyleEnvironment { media, ..StyleEnvironment::default() }),
                        source: previous.is_none_or(|old| old.source != key.layout.source)
                            .then(|| key.layout.source.to_string()),
                        user_styles: previous.is_none_or(|old| old.styles != key.layout.styles)
                            .then(|| key.layout.styles.to_vec()),
                        ..PipelineChanges::default()
                    };
                    document.attach(&mut self.shaper).apply_changes(changes)
                        .map_err(|error| error.to_string())?;
                }
                document
            }
            None => self.document.insert(load_fragment(
                key.layout.source.to_string(), f64::from(width),
                key.layout.styles.to_vec(), &mut self.shaper,
            ).map_err(|error| error.to_string())?),
        };
        let scene = fragment_scene(document).ok_or("fragment width must be positive")?;
        let mut painter = GpuiCommandPainter::commands_only(self.shaper.resources());
        if let Some(previous) = self.display.take() {
            painter = painter.reusing(previous.scene);
        }
        if let Some(color) = key.text_color {
            painter = painter.text_color(color);
        }
        scene.paint(&mut painter);
        let height = scene.content_height() as f32;
        self.display = Some(FragmentDisplay {
            key,
            scene: painter.finish(),
            height,
        });
        Ok(size(px(width), px(height)))
    }

    fn prepare_selection(&mut self) {
        // Flex layout can measure several widths before returning to the same
        // final width. Those probes must not clear an ongoing selection.
        let layout = self.display.as_ref().map(|display| &display.key.layout);
        if self.selection_key.as_ref() != layout {
            if let Some(selection) = &mut self.selection {
                selection.clear();
                self.selection_key = layout.cloned();
            }
        }
    }

    fn selection_context(&mut self) -> Option<(&mut FragmentSelection, RenderView<'_>)> {
        self.display.as_ref()?;
        Some((
            self.selection.as_mut()?,
            self.document.as_ref()?.render_view()?,
        ))
    }
}


/// Native continuous fragments own source replacement and stylesheet updates.
pub(crate) fn load_fragment(
    source: String,
    width: f64,
    styles: Vec<String>,
    shaper: &mut impl GlyphShaper,
) -> Result<PipelineSession, PipelineError> {
    let media = MediaEnvironment::screen(width, None)
        .ok_or(LayoutConstraintError::InvalidViewportWidth)?;
    let mut inputs = PipelineInputs::new(
        source, MarkupSyntax::Html, "fragment.xhtml".to_owned(),
        StyleEnvironment { media, ..StyleEnvironment::default() },
        LayoutConstraints::new(width, 16.0)?,
    );
    inputs.user_styles = styles;
    let mut document = PipelineSession::new(Arc::new(NoResources));
    document.attach(shaper).update(inputs)?;
    Ok(document)
}

fn fragment_scene(document: &PipelineSession) -> Option<RenderScene> {
    let inputs = document.inputs()?;
    let options = SceneOptions::new(inputs.layout.viewport_width(), inputs.layout.viewport_height())
        .ok()?.with_image_policy(SceneImagePolicy::Omit);
    Some(render_core::render_scene_for_view(document.render_view()?, &options))
}

impl IntoElement for HtmlFragmentElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for HtmlFragmentElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let state = window.use_keyed_state("html-fragment-state", cx, |window, cx| {
            FragmentState::new(window, cx)
        });
        state.update(cx, |state, cx| {
            if self.selection_color.is_some() && state.selection.is_none() {
                state.selection = Some(FragmentSelection::new(cx.focus_handle()));
            }
        });
        self.state = Some(state.clone());
        let measured = state;
        let source = self.source.clone();
        let styles: Arc<[String]> = self.styles.clone().into();
        let text_color = self.text_color;
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.flex_shrink = 0.0;
        let layout = window.request_measured_layout(style, move |known, available, window, cx| {
            let width = known.width.unwrap_or_else(|| match available.width {
                AvailableSpace::Definite(width) => width,
                _ => window.viewport_size().width,
            });
            let key = FragmentKey {
                layout: FragmentLayoutKey {
                    source: source.clone(),
                    width: f32::from(width).max(1.0),
                    styles: styles.clone(),
                },
                text_color,
            };
            measured.update(cx, |state, _| {
                state.layout(key).unwrap_or_else(|error| {
                    eprintln!("HTML fragment layout failed: {error}");
                    size(width, px(0.0))
                })
            })
        });
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.state.as_ref()?.update(cx, |state, _| {
            if let Some(mut key) = state.display.as_ref().map(|display| display.key.clone()) {
                key.layout.width = f32::from(bounds.size.width).max(1.0);
                if let Err(error) = state.layout(key) {
                    eprintln!("HTML fragment layout failed: {error}");
                }
            }
            if self.selection_color.is_some() {
                state.prepare_selection();
            }
        });
        if self.selection_color.is_some() {
            let state = self.state.as_ref()?.read(cx);
            state.display.as_ref()?;
            window.set_focus_handle(&state.selection.as_ref()?.focus, cx);
            Some(window.insert_hitbox(bounds, HitboxBehavior::Normal))
        } else {
            None
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let scene = state
            .read(cx)
            .display
            .as_ref()
            .map(|display| display.scene.clone());
        let Some(scene) = scene else {
            return;
        };
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            scene.paint(bounds, window, cx)
        });
        let Some(hitbox) = hitbox else { return };
        let Some(color) = self.selection_color else {
            return;
        };
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let state = state.read(cx);
            let Some(selection) = state.selection.as_ref() else {
                return;
            };
            let Some(document) = state
                .document
                .as_ref()
                .and_then(PipelineSession::render_view)
            else {
                return;
            };
            for rect in selection.rects(document) {
                window.paint_quad(fill(
                    Bounds::new(
                        bounds.origin + point(px(rect.x0 as f32), px(rect.y0 as f32)),
                        size(px(rect.width() as f32), px(rect.height() as f32)),
                    ),
                    color,
                ));
            }
        });
        window.set_cursor_style(CursorStyle::IBeam, hitbox);

        let target = state.clone();
        let hit = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if event.button != MouseButton::Left {
                return;
            }
            if phase.capture() && !hit.is_hovered(window) {
                target.update(cx, |state, cx| {
                    let Some((selection, document)) = state.selection_context() else {
                        return;
                    };
                    if !selection.is_empty(document) || selection.dragging() {
                        selection.clear();
                        cx.notify();
                    }
                });
            } else if phase.bubble() && hit.is_hovered(window) {
                target.update(cx, |state, cx| {
                    let Some((selection, document)) = state.selection_context() else {
                        return;
                    };
                    selection.begin(document, event.position - bounds.origin, event.click_count);
                    selection.focus.focus(window, cx);
                    window.prevent_default();
                    cx.stop_propagation();
                    cx.notify();
                });
            }
        });
        let target = state.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if !phase.bubble() {
                return;
            }
            target.update(cx, |state, cx| {
                let Some((selection, document)) = state.selection_context() else {
                    return;
                };
                if selection.dragging() {
                    if event.pressed_button == Some(MouseButton::Left) {
                        selection.extend(document, event.position - bounds.origin);
                        cx.stop_propagation();
                    } else {
                        selection.finish(document);
                    }
                    cx.notify();
                }
            });
        });
        let target = state.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if !phase.bubble() || event.button != MouseButton::Left {
                return;
            }
            target.update(cx, |state, cx| {
                let Some((selection, document)) = state.selection_context() else {
                    return;
                };
                if selection.dragging() {
                    selection.extend(document, event.position - bounds.origin);
                    selection.finish(document);
                    cx.notify();
                    cx.stop_propagation();
                }
            });
        });
        window.on_key_event(move |event: &KeyDownEvent, phase, window, cx| {
            if !phase.bubble()
                || !state
                    .read(cx)
                    .selection
                    .as_ref()
                    .is_some_and(|selection| selection.focus.is_focused(window))
            {
                return;
            }
            state.update(cx, |state, cx| {
                let Some((selection, document)) = state.selection_context() else {
                    return;
                };
                let modifiers = event.keystroke.modifiers;
                let shortcut = if cfg!(target_os = "macos") {
                    modifiers.platform
                } else {
                    modifiers.control
                };
                match event.keystroke.key.as_str() {
                    "c" if shortcut && !selection.is_empty(document) => cx.write_to_clipboard(
                        ClipboardItem::new_string(selection.selected_text(document)),
                    ),
                    "a" if shortcut => selection.select_all(document),
                    "escape" => selection.clear(),
                    _ => return,
                }
                cx.stop_propagation();
                window.prevent_default();
                cx.notify();
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use html_pipeline::engine::box_queries;

    use super::*;
    use html::layout::{BoxId, RenderView};
    use gpui::{
        Context, EmptyView, Modifiers, ParentElement, Render, Styled, TestAppContext, div, point,
        rgba,
    };

    fn description() -> SharedString {
        format!(
            "<h2>A book</h2><p>{}</p><ul><li>First &amp; second</li><li><em>Another entry</em></li></ul>",
            "A thoughtful description follows the characters through a changing world, with friendships and unexpected discoveries along the way. ".repeat(12)
        )
        .into()
    }

    struct SelectableDescription;

    impl Render for SelectableDescription {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(300.)).child(
                HtmlFragmentElement::new("description", "<p>Hello &amp; café</p><p>Second paragraph</p>")
                    .stylesheet("html, body { margin:0; padding:0; font-size:16px; line-height:24px; } p { margin:0; }")
                    .justify()
                    .selectable(rgba(0x88aa8866).into()),
            )
        }
    }

    #[gpui::test]
    fn description_drag_selection_and_clipboard_shortcuts(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| SelectableDescription);
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let from = point(px(1.), px(12.));
        let to = point(px(48.), px(12.));
        cx.simulate_mouse_move(from, None, Modifiers::default());
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
        let copy = if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        };
        cx.simulate_keystrokes(copy);
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap().trim(),
            "Hello"
        );
        cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-a cmd-c"
        } else {
            "ctrl-a ctrl-c"
        });
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().unwrap(),
            "Hello & café\n\nSecond paragraph"
        );

        cx.simulate_click(point(px(350.), px(150.)), Modifiers::default());
        cx.write_to_clipboard(ClipboardItem::new_string("outside".to_owned()));
        cx.simulate_keystrokes(copy);
        assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), "outside");
    }

    #[gpui::test]
    fn selection_keeps_unicode_and_is_invalidated_by_reflow(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, cx| {
                let mut state = FragmentState::new(window, cx);
                state.selection = Some(FragmentSelection::new(cx.focus_handle()));
                let source = "<p>First &amp; <em>café</em> Åsa</p><p>Second paragraph</p>";
                state.layout(key(source.into(), 300.)).unwrap();
                state.prepare_selection();
                let selection = state.selection.as_mut().unwrap();
                let document = state
                    .document
                    .as_ref()
                    .and_then(PipelineSession::render_view)
                    .unwrap();
                selection.select_all(document);
                assert_eq!(
                    selection.selected_text(document),
                    "First & café Åsa\n\nSecond paragraph"
                );
                let glyph = (0..document.text().glyph_count() as u32)
                    .find(|&index| {
                        text_paint::render_text_view_character_at(document.text(), index as usize)
                            == Some('é')
                    })
                    .unwrap();
                let rect = geometry_queries::selection_bounds_lines(
                    geometry_queries::render_view_geometry(document),
                    glyph..glyph + 1,
                    document
                        .text()
                        .lines()
                        .iter()
                        .map(|line| html::render::LinePlacement {
                            line: line.id(),
                            origin: line.point(),
                        })
                        .into_iter()
                        .map(|placement| (placement.line, placement.origin)),
                )
                .unwrap();
                let position = point(px(rect.center().x as f32), px(rect.center().y as f32));
                assert!(selection.begin(document, position, 2));
                assert_eq!(selection.selected_text(document), "café");
                state.layout(key(source.into(), 110.)).unwrap();
                state.prepare_selection();
                let selection = state.selection.as_mut().unwrap();
                let document = state
                    .document
                    .as_ref()
                    .and_then(PipelineSession::render_view)
                    .unwrap();
                assert!(selection.is_empty(document));
                selection.select_all(document);
                assert_eq!(
                    selection.selected_text(document),
                    "First & café Åsa\n\nSecond paragraph"
                );
                state
                    .layout(key("<p>Replacement</p>".into(), 110.))
                    .unwrap();
                state.prepare_selection();
                let document = state
                    .document
                    .as_ref()
                    .and_then(PipelineSession::render_view)
                    .unwrap();
                let selection = state.selection.as_mut().unwrap();
                selection.select_all(document);
                assert_eq!(selection.selected_text(document), "Replacement");
            })
            .unwrap();
    }

    /// Lay out a fragment at 300px with only the supplied styles.
    fn render<'a>(state: &'a mut FragmentState, source: &str, styles: Vec<String>) -> RenderView<'a> {
        state
            .layout(FragmentKey {
                layout: FragmentLayoutKey {
                    source: source.to_owned().into(),
                    width: 300.0,
                    styles: styles.into(),
                },
                text_color: None,
            })
            .unwrap();
        state.document.as_ref().and_then(PipelineSession::render_view).unwrap()
    }

    fn key(source: SharedString, width: f32) -> FragmentKey {
        let element = HtmlFragmentElement::new("test", source.clone())
            .stylesheet("html, body { margin: 0; font-size: 16px; line-height: 24px; }")
            .justify();
        FragmentKey {
            layout: FragmentLayoutKey {
                source,
                width,
                styles: element.styles.into(),
            },
            text_color: element.text_color,
        }
    }

    #[gpui::test]
    fn fragments_reflow_to_natural_height_and_reuse_unchanged_paint(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, cx| {
                let mut state = FragmentState::new(window, cx);
                let source = description();
                let wide = state.layout(key(source.clone(), 420.0)).unwrap();
                assert_eq!(wide.width, px(420.0));
                assert!(wide.height > px(24.0));
                assert!(!state.display.as_ref().unwrap().scene.is_empty());

                let commands = state.display.as_ref().unwrap().scene.commands.clone();
                assert_eq!(state.layout(key(source.clone(), 420.0)).unwrap(), wide);
                assert!(Arc::ptr_eq(&commands, &state.display.as_ref().unwrap().scene.commands));

                let narrow = state.layout(key(source.clone(), 210.0)).unwrap();
                assert!(
                    narrow.height > wide.height,
                    "narrow columns must grow vertically instead of paginating or clipping"
                );
                assert!(!Arc::ptr_eq(&commands, &state.display.as_ref().unwrap().scene.commands));

                let changed = state
                    .layout(key("<p>A short replacement.</p>".into(), 210.0))
                    .unwrap();
                assert!(
                    changed.height < narrow.height,
                    "a changed description must invalidate its retained scene"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn justified_fragments_distribute_word_spacing_without_height_clipping(
        cx: &mut TestAppContext,
    ) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, cx| {
                let mut state = FragmentState::new(window, cx);
                let source = description();
                state.layout(key(source, 240.0)).unwrap();
                let scene = fragment_scene(state.document.as_ref().unwrap()).unwrap();
                assert!(!render_core::render_scene_is_clipped(&scene));
                assert_eq!(scene.size().height, scene.content_height());
                let document = state
                    .document
                    .as_ref()
                    .and_then(PipelineSession::render_view)
                    .unwrap();
                assert!(
                    document
                        .text()
                        .lines()
                        .iter()
                        .any(|line| text_paint::line_word_spacing(&line) != 0.0),
                    "justified paragraphs must distribute spacing between words"
                );
                assert!(document.text().lines().len() > 2);
            })
            .unwrap();
    }

    #[gpui::test]
    fn default_color_preserves_explicit_and_inherited_css_colors(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, cx| {
                let mut state = FragmentState::new(window, cx);
                let styles = vec![":where(html) { color: #eeeeee; }".to_owned()];
                let source = "<p id='default'>Default</p><p style='color:#008000'><span id='inherited'>Inherited green</span></p><p id='explicit' style='color:#ff0000'>Explicit red</p>";
                let boxes = render(&mut state, source, styles.clone()).boxes();
                let color = |id| box_queries::text_format(boxes, (0..box_queries::len(boxes)).map(layout_state::box_id_new).find(|&index| box_queries::attribute(boxes, index, "id") == Some(id)).unwrap()).color;
                assert_eq!(color("default"), 0xeeeeeeff);
                assert_eq!(color("inherited"), 0x008000ff);
                assert_eq!(color("explicit"), 0xff0000ff);

                let boxes = render(&mut state, "<style>html {color:#123456}</style><p id='root'>Authored root color</p>", styles).boxes();
                let paragraph = (0..box_queries::len(boxes)).map(layout_state::box_id_new).find(|&index| box_queries::attribute(boxes, index, "id") == Some("root")).unwrap();
                assert_eq!(box_queries::text_format(boxes, paragraph).color, 0x123456ff);
            })
            .unwrap();
    }

    #[gpui::test]
    fn typography_is_computed_from_html_and_supplied_css(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, cx| {
                let mut state = FragmentState::new(window, cx);
                let styles = vec![":where(body) { font-size:18px; line-height:1.5; }".to_owned()];
                let boxes = render(&mut state, "<p id='default'>Default</p><p id='authored' style='font-size:24px;font-weight:bold;font-style:italic'>Publisher</p>", styles).boxes();
                let format = |id| box_queries::text_format(boxes, (0..box_queries::len(boxes)).map(layout_state::box_id_new).find(|&index| box_queries::attribute(boxes, index, "id") == Some(id)).unwrap());
                assert_eq!(format("default").font_size, 18.0);
                assert_eq!(format("default").font_weight, 400);
                assert_eq!(format("authored").font_size, 24.0);
                assert_eq!(format("authored").font_weight, 700);
                assert_eq!(format("authored").font_style, html_style_model::FontStyle::Italic);
            })
            .unwrap();
    }
}
