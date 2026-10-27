//! Wry child webview and GLib event integration for the shared GPUI host.

use crate::common::{Options, emit};
use anyhow::{Context, Result, bail, ensure};
use cairo::ImageSurface;
use gpui::{AsyncApp, Task, Window};
use gtk::glib::{self, MainContext, ffi, translate::ToGlibPtr};
use gtk::prelude::GtkSettingsExt;
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, WindowHandle, XlibWindowHandle,
};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::cell::Cell;
use std::fs::File;
use std::io;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;
use url::Url;
use webkit2gtk::{SnapshotOptions, SnapshotRegion, WebViewExt};
use wry::{WebView, WebViewBuilder, WebViewExtUnix};

const READY: &str = r#"
window.addEventListener('DOMContentLoaded', async () => {
  if (location.protocol !== 'skrift-bench:') return;
  try {
    await document.fonts.ready;
    await Promise.all(Array.from(document.images, image => image.decode()));
    window.ipc.postMessage(JSON.stringify({
      event: 'document_ready', width: innerWidth, height: innerHeight,
      scale: devicePixelRatio, layout_width: document.documentElement.clientWidth,
      content_width: document.documentElement.scrollWidth,
      content_height: Math.max(document.documentElement.scrollHeight, document.body.scrollHeight),
      scroll_y: window.scrollY, compatibility:document.compatMode
    }));
  } catch (error) {
    window.ipc.postMessage(JSON.stringify({event:'error', message:String(error)}));
  }
}, {once:true});
"#;

struct NativeParent<'a>(&'a Window);

impl HasWindowHandle for NativeParent<'_> {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        let raw = match HasWindowHandle::window_handle(self.0)?.as_raw() {
            RawWindowHandle::Xcb(handle) => {
                XlibWindowHandle::new(handle.window.get().into()).into()
            }
            RawWindowHandle::Xlib(handle) => handle.into(),
            _ => return Err(HandleError::Unavailable),
        };
        // The GPUI window owns this X11 ID throughout the borrow. GTK uses
        // its own connection to the same display; no display pointer is cast.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

pub struct WryContent {
    _webview: WebView,
    native: webkit2gtk::WebView,
    began: Rc<Cell<Option<Instant>>>,
    pending: Rc<Cell<bool>>,
    phase: Rc<Cell<&'static str>>,
}

impl WryContent {
    pub fn new(window: &Window, options: &Options) -> Result<Self> {
        gtk::init().context("initialize GTK for Wry")?;
        gtk::Settings::default()
            .context("missing GTK settings")?
            .set_gtk_xft_dpi(96 * 1024);
        gtk::gdk::Screen::default()
            .context("missing GDK screen")?
            .set_resolution(96.0);
        let parent = NativeParent(window);
        let RawWindowHandle::Xlib(parent_handle) = parent.window_handle()?.as_raw() else {
            bail!("Wry shared host requires an X11 parent");
        };
        let parent_xid = parent_handle.window;
        let native_cell: Rc<glib::WeakRef<webkit2gtk::WebView>> = Rc::default();
        let callback_native = native_cell.clone();
        let began = Rc::new(Cell::new(None));
        let callback_began = began.clone();
        let pending = Rc::new(Cell::new(false));
        let callback_pending = pending.clone();
        let phase = Rc::new(Cell::new("document"));
        let callback_phase = phase.clone();
        let callback_options = options.clone();
        let resource_root = options
            .fixture
            .parent()
            .context("fixture has no parent")?
            .to_owned();
        let webview = WebViewBuilder::new()
            .with_bounds(wry::Rect {
                position: wry::dpi::LogicalPosition::new(0, 0).into(),
                size: wry::dpi::LogicalSize::new(options.width, options.height).into(),
            })
            .with_custom_protocol("skrift-bench".into(), move |_, request| {
                match local_resource(&resource_root, &request.uri().to_string()) {
                    Ok((bytes, mime)) => wry::http::Response::builder().header("Content-Type", mime)
                        .body(Cow::Owned(bytes)).expect("resource response"),
                    Err(error) => wry::http::Response::builder().status(404)
                        .body(Cow::Owned(error.to_string().into_bytes())).expect("error response"),
                }
            })
            .with_initialization_script_for_main_only(READY, true)
            .with_ipc_handler(move |request| {
                let result = (|| -> Result<()> {
                    let metadata: Value = serde_json::from_str(request.body())?;
                    ensure!(metadata["event"] != "error", "page readiness failed: {}", metadata["message"]);
                    if callback_pending.replace(true) { return Ok(()); }
                    ensure!(metadata["width"].as_u64() == Some(callback_options.width.into())
                        && metadata["height"].as_u64() == Some(callback_options.height.into()),
                        "Wry child viewport mismatch: {metadata}");
                    let began: Instant = callback_began.get().context("page ready before document load")?;
                    let native = callback_native.upgrade().context("missing native WebView")?;
                    let screenshot = callback_options.screenshot.clone();
                    let phase = callback_phase.get();
                    native.snapshot(SnapshotRegion::Visible, SnapshotOptions::NONE,
                        None::<&gtk::gio::Cancellable>, move |snapshot| {
                        let snapshot_at = Instant::now();
                        let result = (|| -> Result<Value> {
                            let surface = ImageSurface::try_from(snapshot?)
                                .map_err(|_| anyhow::anyhow!("snapshot is not an image surface"))?;
                            let dimensions = (surface.width(), surface.height());
                            if let Some(path) = screenshot { surface.write_to_png(&mut File::create(path)?)?; }
                            Ok(json!({"event":"ready", "phase":phase, "engine":"wry-in-gpui",
                                "pid":std::process::id(), "webview_version":wry::webview_version()?,
                                "parent_x11_window":parent_xid, "embedded_as_child":true,
                                "document_to_snapshot_ms":(snapshot_at-began).as_secs_f64()*1000.0,
                                "snapshot_width":dimensions.0, "snapshot_height":dimensions.1,
                                "page":metadata,
                                "endpoint":"WebKit child viewport snapshot complete; includes readback; excludes PNG encoding"}))
                        })();
                        match result {
                            Ok(event) => emit(event),
                            Err(error) => emit(json!({"event":"error", "message":error.to_string()})),
                        }
                    });
                    Ok(())
                })();
                if let Err(error) = result { emit(json!({"event":"error", "message":error.to_string()})); }
            })
            .build_as_child(&parent)?;
        let native = webview.webview();
        native.set_background_color(&gtk::gdk::RGBA::WHITE);
        let settings = native.settings().context("missing WebKit settings")?;
        webkit2gtk::SettingsExt::set_enable_developer_extras(&settings, false);
        native_cell.set(Some(&native));
        Ok(Self {
            _webview: webview,
            native,
            began,
            pending,
            phase,
        })
    }

    pub fn load(&self, options: &Options, began: Instant) -> Result<()> {
        self.began.set(Some(began));
        let source = std::fs::read_to_string(&options.fixture)?;
        let mut base = Url::parse("skrift-bench://fixture/")?;
        base.set_path(
            options
                .fixture
                .file_name()
                .context("fixture has no filename")?
                .to_str()
                .context("fixture filename is not UTF-8")?,
        );
        self.native.load_html(&source, Some(base.as_str()));
        Ok(())
    }

    /// Load another document into the same initialized WebView.
    pub fn load_source(
        &self,
        options: &Options,
        source: &str,
        xml: bool,
        phase: &'static str,
    ) -> Result<()> {
        self.began.set(Some(Instant::now()));
        self.phase.set(phase);
        self.pending.set(false);
        let mut base = Url::parse("skrift-bench://fixture/")?;
        base.set_path(
            options
                .fixture
                .file_name()
                .context("fixture filename")?
                .to_str()
                .context("UTF-8 filename")?,
        );
        self.native.load_bytes(
            &glib::Bytes::from(source.as_bytes()),
            Some(if xml {
                "application/xhtml+xml"
            } else {
                "text/html"
            }),
            Some("UTF-8"),
            Some(base.as_str()),
        );
        Ok(())
    }

    pub fn set_default_font(&self, family: &str) -> Result<()> {
        let settings = self.native.settings().context("WebKit settings")?;
        webkit2gtk::SettingsExt::set_default_font_family(&settings, family);
        webkit2gtk::SettingsExt::set_default_font_size(&settings, 16);
        Ok(())
    }

    pub fn tile(&self, y: f64) {
        self.began.set(Some(Instant::now()));
        self.phase.set("tile");
        self.pending.set(false);
        let script = format!(
            r#"scrollTo(0,{y});
            requestAnimationFrame(() => requestAnimationFrame(() => window.ipc.postMessage(JSON.stringify({{
              event:'document_ready',width:innerWidth,height:innerHeight,scale:devicePixelRatio,
              layout_width:document.documentElement.clientWidth,content_width:document.documentElement.scrollWidth,
              content_height:Math.max(document.documentElement.scrollHeight,document.body.scrollHeight),
              scroll_y:scrollY,compatibility:document.compatMode
            }}))));"#
        );
        self.native
            .run_javascript(&script, None::<&gtk::gio::Cancellable>, |result| {
                if let Err(error) = result {
                    emit(json!({"event":"error","message":error.to_string()}));
                }
            });
    }

    pub fn capture(&self, path: &Path) {
        let path = path.to_owned();
        self.native.snapshot(SnapshotRegion::Visible, SnapshotOptions::NONE,
            None::<&gtk::gio::Cancellable>, move |snapshot| {
            let result = (|| -> Result<Value> {
                let surface = ImageSurface::try_from(snapshot?)
                    .map_err(|_| anyhow::anyhow!("snapshot is not an image surface"))?;
                surface.write_to_png(&mut File::create(&path)?)?;
                Ok(json!({"event":"capture", "path":path, "width":surface.width(), "height":surface.height()}))
            })();
            match result {
                Ok(event) => emit(event),
                Err(error) => emit(json!({"event":"error", "message":error.to_string()})),
            }
        });
    }
}

/// Poll GLib's actual descriptors and deadlines on a background worker; all
/// GTK preparation and dispatch stay on the GPUI UI thread. No periodic pump.
pub struct GtkBridge {
    stop: Rc<Cell<bool>>,
    context: MainContext,
    task: Task<()>,
}

impl GtkBridge {
    pub fn start(cx: &AsyncApp) -> Self {
        let stop = Rc::new(Cell::new(false));
        let stopped = stop.clone();
        let context = MainContext::default();
        let dispatch_context = context.clone();
        let task = cx.spawn(async move |cx| {
            let result = async {
                let _guard = dispatch_context
                    .acquire()
                    .context("acquire GTK main context")?;
                let pointer = dispatch_context.to_glib_none().0;
                while !stopped.get() {
                    let (ready, priority) = dispatch_context.prepare();
                    let mut timeout = -1;
                    let count = unsafe {
                        ffi::g_main_context_query(
                            pointer,
                            priority,
                            &mut timeout,
                            std::ptr::null_mut(),
                            0,
                        )
                    };
                    let empty = ffi::GPollFD {
                        fd: -1,
                        events: 0,
                        revents: 0,
                    };
                    let mut fds = vec![empty; count.max(0) as usize];
                    loop {
                        let count = unsafe {
                            ffi::g_main_context_query(
                                pointer,
                                priority,
                                &mut timeout,
                                fds.as_mut_ptr(),
                                fds.len() as i32,
                            )
                        };
                        if count as usize <= fds.len() {
                            fds.truncate(count as usize);
                            break;
                        }
                        fds.resize(count as usize, empty);
                    }
                    if ready {
                        timeout = 0;
                    }
                    let (mut fds, result, error) = cx
                        .background_executor()
                        .spawn(async move {
                            let result =
                                unsafe { ffi::g_poll(fds.as_mut_ptr(), fds.len() as u32, timeout) };
                            let error = (result < 0).then(io::Error::last_os_error);
                            (fds, result, error)
                        })
                        .await;
                    if stopped.get() {
                        break;
                    }
                    if result < 0 {
                        let error = error.expect("poll error");
                        if error.kind() == io::ErrorKind::Interrupted {
                            continue;
                        }
                        return Err(error.into());
                    }
                    // The context stays acquired on this UI thread while its
                    // descriptor array is polled by the worker.
                    unsafe {
                        if ffi::g_main_context_check(
                            pointer,
                            priority,
                            fds.as_mut_ptr(),
                            fds.len() as i32,
                        ) != 0
                        {
                            ffi::g_main_context_dispatch(pointer);
                        }
                    }
                }
                Ok::<_, anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                emit(json!({"event":"error", "message":error.to_string()}));
            }
        });
        Self {
            stop,
            context,
            task,
        }
    }

    pub async fn stop(self) {
        self.stop.set(true);
        self.context.wakeup();
        self.task.await;
    }
}

fn local_resource(root: &Path, uri: &str) -> Result<(Vec<u8>, &'static str)> {
    let uri = Url::parse(uri)?;
    let path = root
        .join(
            percent_encoding::percent_decode_str(uri.path())
                .decode_utf8()?
                .trim_start_matches('/'),
        )
        .canonicalize()?;
    ensure!(path.starts_with(root), "resource outside fixture directory");
    let mime = match path
        .extension()
        .and_then(|value| value.to_str())
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
