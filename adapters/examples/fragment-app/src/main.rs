use gpui::{
    App, AppContext, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use gpui_platform::application;
use html_gpui::HtmlFragmentElement;

struct FragmentApp;

impl Render for FragmentApp {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // The application owns the scroll container. The adapter provides the
        // natural-height HTML element and its optional text selection.
        div()
            .id("article-scroll")
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .bg(rgb(0xffffff))
            .child(
                HtmlFragmentElement::new("article", include_str!("article.html"))
                    .stylesheet("body { font-family: sans-serif; font-size: 18px; color: #243044; } p { margin: 0 0 20px; }")
                    .selectable(rgb(0xc8dcff).into()),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(640.0), px(480.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| FragmentApp),
        )
        .expect("open HTML example window");
        cx.activate(true);
    });
}
