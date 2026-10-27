#[path = "../../common.rs"]
mod common;

#[cfg(feature = "wry-in-gpui")]
#[path = "../../gpui_wry.rs"]
mod gpui_wry;

use anyhow::{Context as _, Result, bail, ensure};
#[cfg(feature = "skrift")]
use common::blank_html;
use common::{Options, emit};
#[cfg(feature = "skrift")]
mod skrift_content;
#[cfg(feature = "skrift")]
use gpui::AppContext;
use gpui::{
    AnyWindowHandle, App, Bounds, Context, IntoElement, Render, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, rgb, size,
};
use serde_json::json;
#[cfg(feature = "skrift")]
use skrift_content::DocumentView;
use std::cell::Cell;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::rc::Rc;
#[cfg(feature = "skrift")]
use std::time::Duration;
use std::time::Instant;

struct HostView {
    #[cfg(feature = "skrift")]
    document: Option<DocumentView>,
    #[cfg(feature = "wry-in-gpui")]
    browser: Option<gpui_wry::WryContent>,
}

#[cfg(feature = "skrift")]
impl HostView {
    #[cfg(feature = "skrift")]
    fn load(&mut self, source: &str, options: &Options, window: &Window) -> Result<()> {
        self.document
            .get_or_insert_with(|| DocumentView::new(window))
            .load(source, options)
    }


}

impl Render for HostView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "skrift")]
        if let Some(document) = &self.document {
            return document.element().into_any_element();
        }
        div().size_full().bg(rgb(0xffffff)).into_any_element()
    }
}

#[cfg(feature = "skrift")]
fn snapshot(
    handle: AnyWindowHandle,
    phase: &str,
    began: Instant,
    options: &Options,
    cx: &mut impl AppContext,
) -> Result<()> {
    handle.update(cx, |root, window, cx| -> Result<()> {
        window.draw(cx).clear(cx);
        let image = window.render_to_image()?;
        let elapsed = began.elapsed().as_secs_f64() * 1000.0;
        let viewport = window.viewport_size();
        let scale = window.scale_factor();
        let dimensions = image.dimensions();
        let view = root
            .downcast::<HostView>()
            .map_err(|_| anyhow::anyhow!("unexpected root view"))?;
        let state = view
            .read(cx)
            .document
            .as_ref()
            .context("missing document")?;
        let content_height = state.content_height;
        let image_count = state.image_count;
        if phase == "document"
            && let Some(path) = &options.screenshot
        {
            image.save(path)?;
        }

        drop(image);

        emit(json!({
            "event":"ready", "phase":phase, "engine":"skrift-gpui",
            "gpu":window.gpu_specs(),
            "pid":std::process::id(), "document_to_snapshot_ms":elapsed,
            "snapshot_width":dimensions.0, "snapshot_height":dimensions.1,
            "page":{"width":f32::from(viewport.width), "height":f32::from(viewport.height),
                    "scale":scale, "layout_width":f32::from(viewport.width),
                    "content_height":content_height, "images_painted":image_count},
            "endpoint":"viewport snapshot complete; includes readback; excludes PNG encoding"
        }));
        Ok(())
    })??;
    Ok(())
}

fn main() -> Result<()> {
    let options = Options::parse()?;
    ensure!(
        options.content == "skrift" || cfg!(feature = "wry-in-gpui"),
        "Wry requires the wry-in-gpui feature"
    );
    ensure!(
        options.content == "skrift" || options.native_window,
        "Wry shared host requires --native-window"
    );
    ensure!(
        options.content != "skrift" || cfg!(feature = "skrift"),
        "Skrift is not included in this build"
    );
    ensure!(
        cfg!(feature = "skrift") || options.native_window,
        "Wry requires --native-window"
    );
    let (sender, receiver) = async_channel::unbounded();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            match line {
                Ok(line) => {
                    if sender.send_blocking(line).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    eprintln!("stdin failed: {error}");
                    return;
                }
            }
        }
    });
    gpui_platform::application().run(move |cx: &mut App| {
        let result = (|| -> Result<()> {
            let native_ready = Rc::new(Cell::new(false));
            let bounds = Bounds::centered(
                None,
                size(px(options.width as f32), px(options.height as f32)),
                cx,
            );
            let window = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: None,
                    is_resizable: false,
                    ..Default::default()
                },
                |_window, cx| cx.new(|_| HostView {
                    #[cfg(feature = "skrift")]
                    document: (!options.native_window).then(|| DocumentView::new(_window)),
                    #[cfg(feature = "wry-in-gpui")]
                    browser: None,
                }),
            )?;
            #[cfg(feature = "skrift")]
            let began = Instant::now();
            #[cfg(feature = "skrift")]
            if !options.native_window {
                let source = if options.startup_document {
                    std::fs::read_to_string(&options.fixture)?
                } else {
                    blank_html().to_owned()
                };
                window.update(cx, |view, window, _| view.load(&source, &options, window))??;
            }
            if options.native_window {
                let native_ready = native_ready.clone();
                window.update(cx, |_, window, _| {
                    // GPUI drains callbacks before drawing the requested frame.
                    // A second callback therefore observes the previous frame
                    // after its draw and platform presentation submission.
                    window.on_next_frame(move |window, _| {
                        window.on_next_frame(move |window, _| {
                            native_ready.set(true);
                            let viewport = window.viewport_size();
                            let scale = window.scale_factor();
                            emit(json!({
                                "event":"ready", "phase":"native", "engine":"gpui",
                                "pid":std::process::id(), "skrift_initialized":false,
                                "wry_initialized":false,
                                "gpu":window.gpu_specs(),
                                "page":{"width":f32::from(viewport.width),
                                    "height":f32::from(viewport.height), "scale":scale},
                                "frame_width":(f32::from(viewport.width)*scale).round() as u32,
                                "frame_height":(f32::from(viewport.height)*scale).round() as u32,
                                "endpoint":"native GPUI frame drawn and submitted; no Skrift document created"
                            }));
                        });
                    });
                })?;
            }
            cx.activate(true);
            cx.spawn(async move |cx| {
                #[cfg(feature = "wry-in-gpui")]
                let mut gtk_bridge = None;
                let result = async {
                    #[cfg(feature = "skrift")]
                    if options.startup_document {
                        snapshot(window.into(), "document", began, &options, cx)?;
                    } else if !options.native_window {
                        // Allow initial configure and scale to arrive before
                        // capturing the untimed empty-window baseline.
                        cx.background_executor()
                            .timer(Duration::from_millis(100))
                            .await;
                        snapshot(window.into(), "blank", Instant::now(), &options, cx)?;
                    }
                    let mut loaded = options.startup_document;
                    while let Ok(command) = receiver.recv().await {
                        match command.as_str() {
                            "load" if !loaded => {
                                ensure!(!options.native_window || native_ready.get(), "load before native window readiness");

                                let began = Instant::now();
                                #[cfg(feature = "wry-in-gpui")]
                                if options.content == "wry" {
                                    window.update(cx, |view, window, _| -> Result<()> {
                                        view.browser = Some(gpui_wry::WryContent::new(window, &options)?);
                                        Ok(())
                                    })??;
                                    gtk_bridge = Some(gpui_wry::GtkBridge::start(cx));
                                    window.update(cx, |view, _, _| {
                                        view.browser.as_ref().context("missing Wry")?.load(&options, began)
                                    })??;
                                    loaded = true;
                                    continue;
                                }
                                #[cfg(feature = "skrift")]
                                {
                                let source = std::fs::read_to_string(&options.fixture)?;
                                window.update(cx, |view, window, cx| {
                                    view.load(&source, &options, window)?;
                                    cx.notify();
                                    Ok::<_, anyhow::Error>(())
                                })??;
                                snapshot(window.into(), "document", began, &options, cx)?;
                                loaded = true;
                                }
                            }
                            command if command.starts_with("capture ") && loaded => {
                                let path = PathBuf::from(&command[8..]);
                                #[cfg(feature = "wry-in-gpui")]
                                if options.content == "wry" {
                                    window.update(cx, |view, _, _| {
                                        view.browser.as_ref().expect("loaded browser").capture(&path);
                                    })?;
                                    continue;
                                }
                                let handle: AnyWindowHandle = window.into();
                                handle.update(cx, |_, window, cx| -> Result<()> {
                                    window.draw(cx).clear(cx);
                                    let image = window.render_to_image()?;
                                    image.save(&path)?;
                                    emit(json!({"event":"capture", "path":path,
                                        "width":image.width(), "height":image.height()}));
                                    Ok(())
                                })??;
                            }
                            "quit" => break,
                            _ => bail!("unexpected command: {command}"),
                        }
                    }
                    Ok::<_, anyhow::Error>(())
                }
                .await;
                #[cfg(feature = "wry-in-gpui")]
                if let Some(bridge) = gtk_bridge {
                    // Destroy the child before its GPUI parent and wake the
                    // GLib poll worker before executor shutdown.
                    let _ = window.update(cx, |view, _, _| { view.browser.take(); });
                    bridge.stop().await;
                }
                if let Err(error) = result {
                    emit(json!({"event":"error", "message":error.to_string()}));
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
            Ok(())
        })();
        if let Err(error) = result {
            emit(json!({"event":"error", "message":error.to_string()}));
            cx.quit();
        }
    });
    Ok(())
}
