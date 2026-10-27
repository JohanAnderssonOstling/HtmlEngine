// Separate host for initialized-engine and whole-document workloads.
#[path = "../../common.rs"]
mod common;
#[path = "../../gpui_wry.rs"]
mod gpui_wry;
mod skrift_content;

use anyhow::{Context as _, Result, bail, ensure};
use common::{Options, blank_html, emit};
use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, IntoElement, Render, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, rgb, size,
};
use html::pipeline::MarkupSyntax;
use serde_json::json;
use skrift_content::DocumentView;
use std::cell::Cell;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

struct Host {
    document: Option<DocumentView>,
    browser: Option<gpui_wry::WryContent>,
    scroll_y: f32,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        match &self.document {
            Some(document) => document.element_at(self.scroll_y).into_any_element(),
            None => div().size_full().bg(rgb(0xffffff)).into_any_element(),
        }
    }
}

fn snapshot(
    handle: AnyWindowHandle,
    phase: &str,
    began: Instant,
    cx: &mut impl AppContext,
) -> Result<()> {
    handle.update(cx, |root, window, cx| -> Result<()> {
        window.draw(cx).clear(cx);
        let image = window.render_to_image()?;
        let view = root
            .downcast::<Host>()
            .map_err(|_| anyhow::anyhow!("unexpected root"))?;
        let host = view.read(cx);
        let document = host.document.as_ref().context("missing Skrift document")?;
        emit(json!({"event":"ready","phase":phase,"engine":"skrift-gpui",
            "document_to_snapshot_ms":began.elapsed().as_secs_f64()*1000.0,
            "snapshot_width":image.width(),"snapshot_height":image.height(),
            "page":{"width":f32::from(window.viewport_size().width),
                "height":f32::from(window.viewport_size().height),"scale":window.scale_factor(),
                "layout_width":f32::from(window.viewport_size().width),
                "content_height":document.content_height,"scroll_y":host.scroll_y},
            "endpoint":"viewport snapshot complete including readback, without PNG encoding"}));
        Ok(())
    })??;
    Ok(())
}

fn main() -> Result<()> {
    let options = Options::parse()?;
    ensure!(
        options.native_window && !options.startup_document,
        "workload host requires --native-window"
    );
    let (sender, receiver) = async_channel::unbounded();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if sender.send_blocking(line).is_err() {
                break;
            }
        }
    });
    gpui_platform::application().run(move |cx: &mut App| {
        let result = (|| -> Result<()> {
            let native_ready = Rc::new(Cell::new(false));
            let window = cx.open_window(WindowOptions {
                window_bounds:Some(WindowBounds::Windowed(Bounds::centered(None,
                    size(px(options.width as f32),px(options.height as f32)),cx))),
                titlebar:None,is_resizable:false,..Default::default()
            }, |_, cx| cx.new(|_| Host { document:None,browser:None,scroll_y:0.0 }))?;
            let ready = native_ready.clone();
            window.update(cx, |_, window, _| window.on_next_frame(move |window,_| {
                window.on_next_frame(move |window,_| {
                    ready.set(true);
                    let viewport = window.viewport_size();let scale = window.scale_factor();
                    emit(json!({"event":"ready","phase":"native","engine":"gpui",
                        "skrift_initialized":false,"wry_initialized":false,
                        "page":{"width":f32::from(viewport.width),"height":f32::from(viewport.height),"scale":scale},
                        "frame_width":(f32::from(viewport.width)*scale).round() as u32,
                        "frame_height":(f32::from(viewport.height)*scale).round() as u32}));
                });
            }))?;
            cx.activate(true);
            cx.spawn(async move |cx| {
                let mut bridge = None;
                let result = async {
                    while let Ok(command) = receiver.recv().await {
                        match command.as_str() {
                            "warm" | "load" | "load-all" => {
                                ensure!(native_ready.get(),"content requested before native frame");
                                let warm = command == "warm";
                                let phase = if warm { "warm" } else { "document" };
                                let began = Instant::now();
                                let source = if warm { blank_html().to_owned() } else { std::fs::read_to_string(&options.fixture)? };
                                let syntax = if warm { MarkupSyntax::Html } else { MarkupSyntax::from_uri(&options.fixture.to_string_lossy()) };
                                if options.content == "wry" {
                                    let created = window.update(cx, |host, window, _| -> Result<bool> {
                                        if host.browser.is_some() { return Ok(false); }
                                        let browser = gpui_wry::WryContent::new(window,&options)?;
                                        browser.set_default_font("DejaVu Sans")?;
                                        host.browser = Some(browser);
                                        Ok(true)
                                    })??;
                                    if created { bridge = Some(gpui_wry::GtkBridge::start(cx)); }
                                    window.update(cx, |host, _, _| host.browser.as_ref().unwrap()
                                        .load_source(&options,&source,syntax == MarkupSyntax::Xml,phase))??;
                                } else {
                                    window.update(cx, |host, window, cx| -> Result<()> {
                                        let document = host.document.get_or_insert_with(|| DocumentView::new(window));
                                        document.default_font_family = Some("DejaVu Sans".to_owned());
                                        document.markup_syntax = syntax;
                                        document.paint_all = command == "load-all";
                                        document.load(&source,&options)?;
                                        host.scroll_y = 0.0;
                                        cx.notify();Ok(())
                                    })??;
                                    snapshot(window.into(),phase,began,cx)?;
                                }
                            }
                            command if command.starts_with("tile ") => {
                                let y:f64 = command[5..].parse()?;
                                ensure!(y.is_finite() && y >= 0.0,"invalid scroll offset");
                                let began = Instant::now();
                                if options.content == "wry" {
                                    window.update(cx, |host,_,_| host.browser.as_ref().expect("loaded Wry").tile(y))?;
                                } else {
                                    window.update(cx, |host,_,cx| -> Result<()> {
                                        let document = host.document.as_ref().context("loaded Skrift")?;
                                        host.scroll_y = y.min((document.content_height-f64::from(options.height)).max(0.0)) as f32;
                                        cx.notify();Ok(())
                                    })??;
                                    snapshot(window.into(),"tile",began,cx)?;
                                }
                            }
                            command if command.starts_with("capture ") => {
                                let path = PathBuf::from(&command[8..]);
                                if options.content == "wry" {
                                    window.update(cx, |host,_,_| host.browser.as_ref().expect("loaded Wry").capture(&path))?;
                                } else {
                                    let handle:AnyWindowHandle = window.into();
                                    handle.update(cx, |_,window,cx| -> Result<()> {
                                        window.draw(cx).clear(cx);let image = window.render_to_image()?;
                                        image.save(&path)?;
                                        emit(json!({"event":"capture","path":path,"width":image.width(),"height":image.height()}));
                                        Ok(())
                                    })??;
                                }
                            }
                            "inspect" => {
                                window.update(cx, |host,_,_| {
                                    emit(json!({"event":"inspect","document":host.document.as_ref().expect("Skrift").inspect()}));
                                })?;
                            }
                            "quit" => break,
                            _ => bail!("unexpected command: {command}"),
                        }
                    }
                    Ok::<_,anyhow::Error>(())
                }.await;
                if let Some(bridge) = bridge {
                    let _ = window.update(cx,|host,_,_| {host.browser.take();});
                    bridge.stop().await;
                }
                if let Err(error) = result { emit(json!({"event":"error","message":format!("{error:#}")})); }
                cx.update(|cx| cx.quit());
            }).detach();
            Ok(())
        })();
        if let Err(error) = result {emit(json!({"event":"error","message":format!("{error:#}")}));cx.quit();}
    });
    Ok(())
}
