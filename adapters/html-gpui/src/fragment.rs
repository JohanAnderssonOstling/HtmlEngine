//! Continuous HTML fragments embedded in ordinary GPUI layouts.

use std::panic::Location;
use std::sync::Arc;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, ContentMask, CursorStyle, Element, ElementId,
    Entity, GlobalElementId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement,
    KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    SharedString, Size, Style, Window, fill, point, px, relative, size,
};
use html::render::{FragmentImagePolicy, FragmentRenderOptions, FragmentRenderer};

use crate::fragment_selection::FragmentSelection;
use crate::paint::{GpuiCommandPainter, PaintCommand, paint_commands};
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
struct FragmentKey {
    source: SharedString,
    width: f32,
    styles: Vec<String>,
    text_color: Option<Hsla>,
}

struct FragmentState {
    renderer: FragmentRenderer,
    shaper: GpuiTextCache,
    key: Option<FragmentKey>,
    commands: Arc<Vec<PaintCommand>>,
    height: f32,
    selection: Option<FragmentSelection>,
    selection_key: Option<FragmentKey>,
}

impl FragmentState {
    fn new(window: &Window) -> Self {
        Self {
            renderer: FragmentRenderer::new(),
            shaper: GpuiTextCache::new(window.text_system().clone()),
            key: None,
            commands: Arc::default(),
            height: 0.0,
            selection: None,
            selection_key: None,
        }
    }

    fn layout(&mut self, key: FragmentKey) -> Result<Size<Pixels>, String> {
        let width = key.width;
        if self.key.as_ref() != Some(&key) {
            let options = FragmentRenderOptions::new(f64::from(key.width), None)
                .map_err(|error| error.to_string())?
                .with_user_styles(key.styles.clone())
                .with_image_policy(FragmentImagePolicy::Omit);
            let rendered = self
                .renderer
                .render(&key.source, &options, &mut self.shaper)
                .map_err(|error| error.to_string())?;
            let commands = Arc::try_unwrap(std::mem::take(&mut self.commands)).unwrap_or_default();
            let mut painter = GpuiCommandPainter::commands_only(
                self.shaper.store(),
                self.shaper.run_store(),
                commands,
            );
            if let Some(color) = key.text_color {
                painter = painter.text_color(color);
            }
            rendered.scene().paint(&mut painter);
            self.commands = Arc::new(painter.into_commands());
            self.height = rendered.scene().content_height() as f32;
            self.key = Some(key);
        }
        Ok(size(px(width), px(self.height)))
    }

    fn prepare_selection(&mut self) {
        // Flex layout can measure several widths before returning to the same
        // final width. Those probes must not clear an ongoing selection.
        if self.selection_key != self.key {
            if let Some(selection) = &mut self.selection {
                selection.clear();
                self.selection_key = self.key.clone();
            }
        }
    }
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
        let state = window.use_keyed_state("html-fragment-state", cx, |window, _| {
            FragmentState::new(window)
        });
        state.update(cx, |state, cx| {
            if self.selection_color.is_some() && state.selection.is_none() {
                state.selection = Some(FragmentSelection::new(cx.focus_handle()));
                state.key = None;
            }
        });
        self.state = Some(state.clone());
        let measured = state;
        let source = self.source.clone();
        let styles = self.styles.clone();
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
                source: source.clone(),
                width: f32::from(width).max(1.0),
                styles: styles.clone(),
                text_color,
            };
            measured.update(cx, |state, _| {
                state.layout(key).unwrap_or_else(|error| {
                    state.commands = Arc::default();
                    state.key = None;
                    state.selection_key = None;
                    if let Some(selection) = &mut state.selection {
                        selection.clear();
                    }
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
            if let Some(mut key) = state.key.clone() {
                key.width = f32::from(bounds.size.width).max(1.0);
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
        let (commands, glyphs) = {
            let state = state.read(cx);
            (state.commands.clone(), state.shaper.store())
        };
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            paint_commands(&commands, &glyphs, bounds, window, cx)
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
            let Some(document) = state.renderer.render_view() else {
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
                    let Some(selection) = state.selection.as_mut() else {
                        return;
                    };
                    if !selection.is_empty() || selection.dragging() {
                        selection.clear();
                        cx.notify();
                    }
                });
            } else if phase.bubble() && hit.is_hovered(window) {
                target.update(cx, |state, cx| {
                    let Some(selection) = state.selection.as_mut() else {
                        return;
                    };
                    if let Some(document) = state.renderer.render_view() {
                        selection.begin(
                            document,
                            event.position - bounds.origin,
                            event.click_count,
                        );
                    }
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
                let Some(selection) = state.selection.as_mut() else {
                    return;
                };
                if selection.dragging() {
                    if event.pressed_button == Some(MouseButton::Left) {
                        if let Some(document) = state.renderer.render_view() {
                            selection.extend(document, event.position - bounds.origin);
                        }
                        cx.stop_propagation();
                    } else {
                        selection.finish();
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
                let Some(selection) = state.selection.as_mut() else {
                    return;
                };
                if selection.dragging() {
                    if let Some(document) = state.renderer.render_view() {
                        selection.extend(document, event.position - bounds.origin);
                    }
                    selection.finish();
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
                let Some(selection) = state.selection.as_mut() else {
                    return;
                };
                let modifiers = event.keystroke.modifiers;
                let shortcut = if cfg!(target_os = "macos") {
                    modifiers.platform
                } else {
                    modifiers.control
                };
                match event.keystroke.key.as_str() {
                    "c" if shortcut && !selection.is_empty() => {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            state
                                .renderer
                                .render_view()
                                .map(|view| selection.selected_text(view))
                                .unwrap_or_default(),
                        ))
                    }
                    "a" if shortcut => {
                        if let Some(document) = state.renderer.render_view() {
                            selection.select_all(document);
                        }
                    }
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
    use super::*;
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
                let mut state = FragmentState::new(window);
                state.selection = Some(FragmentSelection::new(cx.focus_handle()));
                let source = "<p>First &amp; <em>café</em> Åsa</p><p>Second paragraph</p>";
                state.layout(key(source.into(), 300.)).unwrap();
                state.prepare_selection();
                let selection = state.selection.as_mut().unwrap();
                let document = state.renderer.render_view().unwrap();
                selection.select_all(document);
                assert_eq!(
                    selection.selected_text(document),
                    "First & café Åsa\n\nSecond paragraph"
                );
                let glyph = (0..document.text().glyph_count() as u32)
                    .find(|&index| {
                        document
                            .text()
                            .glyph_at(index as usize)
                            .and_then(|glyph| document.text().glyph_metric(glyph))
                            .is_some_and(|metric| metric.ch() == 'é')
                    })
                    .unwrap();
                let rect = html::geometry::Geometry::new(document)
                    .selection_bounds(glyph..glyph + 1)
                    .unwrap();
                let position = point(px(rect.center().x as f32), px(rect.center().y as f32));
                assert!(selection.begin(document, position, 2));
                assert_eq!(selection.selected_text(document), "café");
                state.layout(key(source.into(), 110.)).unwrap();
                state.prepare_selection();
                let selection = state.selection.as_mut().unwrap();
                assert!(selection.is_empty());
                let document = state.renderer.render_view().unwrap();
                selection.select_all(document);
                assert_eq!(
                    selection.selected_text(document),
                    "First & café Åsa\n\nSecond paragraph"
                );
                state
                    .layout(key("<p>Replacement</p>".into(), 110.))
                    .unwrap();
                state.prepare_selection();
                let document = state.renderer.render_view().unwrap();
                let selection = state.selection.as_mut().unwrap();
                selection.select_all(document);
                assert_eq!(selection.selected_text(document), "Replacement");
            })
            .unwrap();
    }

    fn key(source: SharedString, width: f32) -> FragmentKey {
        let element = HtmlFragmentElement::new("test", source.clone())
            .stylesheet("html, body { margin: 0; font-size: 16px; line-height: 24px; }")
            .justify();
        FragmentKey {
            source,
            width,
            styles: element.styles,
            text_color: element.text_color,
        }
    }

    #[gpui::test]
    fn fragments_reflow_to_natural_height_and_reuse_unchanged_paint(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let mut state = FragmentState::new(window);
                let source = description();
                let wide = state.layout(key(source.clone(), 420.0)).unwrap();
                assert_eq!(wide.width, px(420.0));
                assert!(wide.height > px(24.0));
                assert!(!state.commands.is_empty());

                let commands = state.commands.clone();
                assert_eq!(state.layout(key(source.clone(), 420.0)).unwrap(), wide);
                assert!(Arc::ptr_eq(&commands, &state.commands));

                let narrow = state.layout(key(source.clone(), 210.0)).unwrap();
                assert!(
                    narrow.height > wide.height,
                    "narrow columns must grow vertically instead of paginating or clipping"
                );
                assert!(!Arc::ptr_eq(&commands, &state.commands));

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
            .update(cx, |_, window, _| {
                let mut state = FragmentState::new(window);
                let source = description();
                let options = FragmentRenderOptions::new(240.0, None)
                    .unwrap()
                    .with_user_styles(key(source.clone(), 240.0).styles);
                let rendered = state
                    .renderer
                    .render(&source, &options, &mut state.shaper)
                    .unwrap();
                assert!(!rendered.scene().is_clipped());
                assert_eq!(
                    rendered.scene().size().height,
                    rendered.scene().content_height()
                );
                let document = rendered.render_view();
                assert!(
                    document
                        .text()
                        .lines()
                        .iter()
                        .any(|line| line.word_spacing() != 0.0),
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
            .update(cx, |_, window, _| {
                let mut state = FragmentState::new(window);
                let options = FragmentRenderOptions::new(300.0, None).unwrap().with_user_styles(vec![":where(html) { color: #eeeeee; }".into()]);
                let source = "<p id='default'>Default</p><p style='color:#008000'><span id='inherited'>Inherited green</span></p><p id='explicit' style='color:#ff0000'>Explicit red</p>";
                let rendered = state.renderer.render(source, &options, &mut state.shaper).unwrap();
                let boxes = rendered.render_view().boxes();
                let color = |id| boxes.text_format((0..boxes.len()).find(|&index| boxes.attribute(index, "id") == Some(id)).unwrap()).color;
                assert_eq!(color("default"), 0xeeeeeeff);
                assert_eq!(color("inherited"), 0x008000ff);
                assert_eq!(color("explicit"), 0xff0000ff);

                let rendered = state.renderer.render("<style>html {color:#123456}</style><p id='root'>Authored root color</p>", &options, &mut state.shaper).unwrap();
                let boxes = rendered.render_view().boxes();
                let paragraph = (0..boxes.len()).find(|&index| boxes.attribute(index, "id") == Some("root")).unwrap();
                assert_eq!(boxes.text_format(paragraph).color, 0x123456ff);
            })
            .unwrap();
    }

    #[gpui::test]
    fn typography_is_computed_from_html_and_supplied_css(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| EmptyView);
        window
            .update(cx, |_, window, _| {
                let mut state = FragmentState::new(window);
                let options = FragmentRenderOptions::new(300.0, None).unwrap().with_user_styles(vec![":where(body) { font-size:18px; line-height:1.5; }".into()]);
                let rendered = state.renderer.render("<p id='default'>Default</p><p id='authored' style='font-size:24px;font-weight:bold;font-style:italic'>Publisher</p>", &options, &mut state.shaper).unwrap();
                let boxes = rendered.render_view().boxes();
                let format = |id| boxes.text_format((0..boxes.len()).find(|&index| boxes.attribute(index, "id") == Some(id)).unwrap());
                assert_eq!(format("default").font_size, 18.0);
                assert_eq!(format("default").font_weight, 400);
                assert_eq!(format("authored").font_size, 24.0);
                assert_eq!(format("authored").font_weight, 700);
                assert_eq!(format("authored").font_style, html::layout::FontStyle::Italic);
            })
            .unwrap();
    }
}
