//! Native Linux Wry baseline. The timed endpoint is a completed viewport
//! snapshot, including readback, and is deliberately not called presentation.

#[path = "../../common.rs"]
mod common;

use anyhow::{Context, Result, bail};
use cairo::ImageSurface;
use common::{Options, blank_html, emit};
use gtk::prelude::{BoxExt, GtkSettingsExt, WidgetExt};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::fs::File;
use std::io::{self, BufRead};
use std::time::{Duration, Instant};
use tao::dpi::{LogicalPosition, LogicalSize};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::unix::WindowExtUnix;
use tao::window::WindowBuilder;
use url::Url;
use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};
use wry::{WebViewBuilder, WebViewBuilderExtUnix, WebViewExtUnix};

// This is harness instrumentation, not an authored page workload. The native
// snapshot below is the rendering fence, without an artificial frame delay.
const READY: &str = r#"
window.addEventListener('DOMContentLoaded', async () => {
  try {
    await document.fonts.ready;
    await Promise.all(Array.from(document.images, image => image.decode()));
    window.ipc.postMessage(JSON.stringify({
      event: 'document_ready',
      width: innerWidth, height: innerHeight, scale: devicePixelRatio,
      layout_width: document.documentElement.clientWidth,
      content_width: document.documentElement.scrollWidth,
      content_height: document.documentElement.scrollHeight
    }));
  } catch (error) {
    window.ipc.postMessage(JSON.stringify({event:'error', message:String(error)}));
  }
}, {once:true});
"#;

enum Message {
    Command(String),
    Page(Value),
    Snapshot(Value),
    Error(String),
}

fn main() -> Result<()> {
    let options = Options::parse()?;
    let event_loop = EventLoopBuilder::<Message>::with_user_event().build();
    // WebKit otherwise applies the desktop's font DPI on top of GDK's integer
    // window scale, changing CSS viewport dimensions relative to GPUI.
    gtk::Settings::default()
        .context("missing GTK settings")?
        .set_gtk_xft_dpi(96 * 1024);
    gtk::gdk::Screen::default()
        .context("missing GDK screen")?
        .set_resolution(96.0);
    let window = WindowBuilder::new()
        .with_title("Skrift embedding benchmark: Wry / WebKitGTK")
        .with_inner_size(LogicalSize::new(options.width, options.height))
        .with_decorations(false)
        .with_resizable(false)
        .build(&event_loop)?;
    let container = gtk::Fixed::new();
    container.set_size_request(options.width as i32, options.height as i32);
    container.show();
    window
        .default_vbox()
        .context("missing GTK container")?
        .pack_start(&container, false, false, 0);
    let proxy = event_loop.create_proxy();
    let resource_root = options
        .fixture
        .parent()
        .context("fixture has no parent")?
        .to_owned();
    let webview = WebViewBuilder::new()
        .with_bounds(wry::Rect {
            position: LogicalPosition::new(0, 0).into(),
            size: LogicalSize::new(options.width, options.height).into(),
        })
        .with_custom_protocol("skrift-bench".into(), move |_, request| {
            let response = local_resource(&resource_root, &request.uri().to_string());
            match response {
                Ok((bytes, mime)) => wry::http::Response::builder()
                    .header("Content-Type", mime)
                    .body(Cow::Owned(bytes))
                    .expect("static response headers"),
                Err(error) => wry::http::Response::builder()
                    .status(404)
                    .body(Cow::Owned(error.to_string().into_bytes()))
                    .expect("static response status"),
            }
        })
        .with_initialization_script_for_main_only(READY, true)
        .with_ipc_handler(move |request| {
            let message = match serde_json::from_str(request.body()) {
                Ok(value) => Message::Page(value),
                Err(error) => Message::Error(error.to_string()),
            };
            let _ = proxy.send_event(message);
        })
        .build_gtk(&container)?;
    let native = webview.webview();
    native.set_background_color(&gtk::gdk::RGBA::WHITE);
    let settings = WebViewExt::settings(&native).context("missing WebKit settings")?;
    webkit2gtk::SettingsExt::set_enable_developer_extras(&settings, false);
    let version = wry::webview_version()?;
    let mut phase = "blank";
    let mut phase_started = Instant::now();
    let mut pending = false;
    let mut ready = false;
    let mut deadline = Instant::now() + Duration::from_secs(60);
    native.load_html(blank_html(), Some("skrift-bench://fixture/blank.html"));

    let proxy = event_loop.create_proxy();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { return };
            if proxy.send_event(Message::Command(line)).is_err() {
                return;
            }
        }
        let _ = proxy.send_event(Message::Command("quit".into()));
    });

    let snapshot_proxy = event_loop.create_proxy();
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(deadline);
        let result: Result<()> = (|| {
            match event {
                Event::UserEvent(Message::Page(metadata)) => {
                    if metadata["event"] == "error" {
                        bail!("page readiness failed: {}", metadata["message"]);
                    }
                    if pending || ready { return Ok(()); }
                    if metadata["width"].as_u64() != Some(u64::from(options.width))
                        || metadata["height"].as_u64() != Some(u64::from(options.height)) {
                        bail!("viewport differs from requested size: {metadata}");
                    }
                    pending = true;
                    let began = phase_started;
                    let phase_name = phase;
                    let screenshot = (phase == "document").then(|| options.screenshot.clone()).flatten();
                    let version = version.clone();
                    let proxy = snapshot_proxy.clone();
                    native.snapshot(SnapshotRegion::Visible, SnapshotOptions::NONE, None::<&gtk::gio::Cancellable>, move |snapshot| {
                        let snapshot_at = Instant::now();
                        let result: Result<Value> = (|| {
                            let surface = ImageSurface::try_from(snapshot?).map_err(|_| anyhow::anyhow!("snapshot is not an image surface"))?;
                            let dimensions = (surface.width(), surface.height());
                            if let Some(path) = screenshot {
                                surface.write_to_png(&mut File::create(path)?)?;
                            }
                            Ok(json!({
                                "event":"ready", "phase":phase_name, "engine":"wry-webkitgtk",
                                "webview_version":version, "pid":std::process::id(),
                                "document_to_snapshot_ms":(snapshot_at-began).as_secs_f64()*1000.0,
                                "snapshot_width":dimensions.0, "snapshot_height":dimensions.1,
                                "page":metadata,
                                "endpoint":"viewport snapshot complete; includes readback; excludes PNG encoding"
                            }))
                        })();
                        let message = match result {
                            Ok(value) => Message::Snapshot(value),
                            Err(error) => Message::Error(error.to_string()),
                        };
                        let _ = proxy.send_event(message);
                    });
                }
                Event::UserEvent(Message::Snapshot(value)) => {
                    pending = false;
                    ready = true;
                    emit(value);
                }
                Event::UserEvent(Message::Command(command)) if command == "load" => {
                    if phase != "blank" || !ready { bail!("unexpected load command"); }
                    phase = "document";
                    phase_started = Instant::now();
                    pending = false;
                    ready = false;
                    deadline = Instant::now() + Duration::from_secs(60);
                    load_document(&native, &options)?;
                }
                Event::UserEvent(Message::Command(command)) if command == "quit" => *control_flow = ControlFlow::Exit,
                Event::UserEvent(Message::Error(message)) => bail!("{message}"),
                Event::WindowEvent {event:WindowEvent::CloseRequested, ..} => *control_flow = ControlFlow::Exit,
                Event::NewEvents(_) if Instant::now() >= deadline => bail!("rendering or harness command timed out"),
                _ => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            emit(json!({"event":"error","message":error.to_string()}));
            *control_flow = ControlFlow::Exit;
        }
    })
}

fn load_document(view: &webkit2gtk::WebView, options: &Options) -> Result<()> {
    let source = std::fs::read_to_string(&options.fixture)?;
    let mut base = Url::parse("skrift-bench://fixture/")?;
    base.set_path(
        options
            .fixture
            .file_name()
            .context("fixture has no filename")?
            .to_str()
            .context("fixture filename must be UTF-8")?,
    );
    view.load_html(&source, Some(base.as_str()));
    Ok(())
}

fn local_resource(root: &std::path::Path, uri: &str) -> Result<(Vec<u8>, &'static str)> {
    let uri = Url::parse(uri)?;
    let decoded = percent_encoding::percent_decode_str(uri.path()).decode_utf8()?;
    let path = root.join(decoded.trim_start_matches('/')).canonicalize()?;
    if !path.starts_with(root) {
        bail!("resource is outside the fixture directory");
    }
    let mime = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
    {
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "avif" => "image/avif",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    };
    Ok((std::fs::read(path)?, mime))
}
