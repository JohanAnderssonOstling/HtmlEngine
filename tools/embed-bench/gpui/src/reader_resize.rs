//! Native reader resize benchmark host. Commands: resize W H, inspect,
//! navigate HREF, capture PATH, quit. Column width stays fixed at 600px.
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, px, rgb, size,
};
use html_view_gpui::HtmlView;
use serde_json::json;
use skrift_column_paginator::{FileSystemProvider, RendererCommand, RendererInitialConfig};
use std::{
    io::{self, BufRead},
    path::PathBuf,
    sync::Arc,
};

struct Host {
    document: Entity<HtmlView>,
    planned_width: Option<f64>,
}
impl Render for Host {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = f32::from(window.viewport_size().width) as f64;
        if self.planned_width != Some(width) {
            self.planned_width = Some(width);
            self.document.update(cx, |view, cx| {
                view.queue_appearance(
                    RendererCommand::SetColumnViewportPlan {
                        available_width: width,
                        column_width: 600.0,
                        column_count: 1,
                        column_gap: 0.0,
                    },
                    cx,
                )
            });
        }
        div()
            .size_full()
            .bg(rgb(0xffffff))
            .child(self.document.clone())
    }
}
fn main() {
    let path =
        std::fs::canonicalize(std::env::args_os().nth(1).expect("HTML path")).expect("fixture");
    let (sender, receiver) = async_channel::unbounded();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if sender.send_blocking(line).is_err() {
                break;
            }
        }
    });
    gpui_platform::application().run(move |cx: &mut App| {
        if let Some(font) = std::env::var_os("EMBED_BENCH_FALLBACK_FONT_FILE") {
            cx.text_system().add_font_file(&PathBuf::from(font)).expect("font file");
            cx.text_system().set_fallback_font("Droid Sans Fallback").expect("fallback");
        }
        let bounds = Bounds::centered(None, size(px(900.0), px(800.0)), cx);
        let handle = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)), titlebar: None,
            is_resizable: true, ..Default::default()
        }, |window, cx| {
            window.set_window_title("Florence reader resize benchmark");
            let document = cx.new(|cx| HtmlView::from_provider(
                Arc::new(FileSystemProvider::new()), vec![path.to_string_lossy().into_owned()], 0, None,
                RendererInitialConfig { column_width: 600.0, max_column_count: Some(1), font_size: 16.0, ..Default::default() },
                window, cx,
            ).expect("document"));
            cx.new(|_| Host { document, planned_width: None })
        }).expect("window");
        cx.activate(true);
        println!("{}", json!({"event":"ready"}));
        cx.spawn(async move |cx| {
            while let Ok(command) = receiver.recv().await {
                if command == "quit" { cx.update(|cx| cx.quit()); break; }
                handle.update(cx, |host, window, cx| {
                    if let Some(dimensions) = command.strip_prefix("resize ") {
                        let sizes: Vec<f32> = dimensions.split_whitespace().map(|v| v.parse().expect("size")).collect();
                        window.resize(size(px(sizes[0]), px(sizes[1])));
                    } else if let Some(href) = command.strip_prefix("navigate ") {
                        host.document.update(cx, |view, cx| {
                            let success = view.navigate_to_href(href, cx);
                            println!("{}", json!({"event":"navigation", "success":success}));
                        });
                    } else if command == "repaint" {
                        host.document.update(cx, |view, cx| {
                            view.apply(RendererCommand::SetReaderPaintPalette(skrift_column_paginator::ReaderPaintPalette {
                                foreground: Some(0x000000ff), background: None,
                            }), cx);
                            view.apply(RendererCommand::SetReaderPaintPalette(Default::default()), cx);
                        });
                    } else if command == "inspect" {
                        let view = host.document.read(cx);
                        let geometry = view.layout_geometry();
                        println!("{}", json!({"event":"inspect", "compositions":view.paint_stats().compositions,
                            "page_key":format!("{:?}",view.visible_page_key()),
                            "geometry":geometry.map(|g| json!({"width":g.viewport_width,"height":g.viewport_height,"column_width":g.column_width,"columns":g.column_count})),
                            "glyph_shapes":view.text_shape_cache_stats().glyph_shapes,
                            "line_shapes":view.text_shape_cache_stats().line_shapes}));
                    } else if let Some(path) = command.strip_prefix("capture ") {
                        window.render_to_image().expect("snapshot").save(path).expect("PNG");
                        println!("{}", json!({"event":"capture"}));
                    }
                }).expect("command");
            }
        }).detach();
    });
}
