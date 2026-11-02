//! Wry child webview and GLib event integration for the shared GPUI host.

use crate::common::{Options, PhaseTimings, emit};
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
use std::cell::{Cell, RefCell};
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
    if (new URL(location.href).searchParams.get('skrift-bench-scroll') === '1') {
      skriftBenchScrollDocument();
      return;
    }
    const documentFrames = new URL(location.href).searchParams.get('skrift-bench-frames') === '1';
    // Force the final layout before giving the document two paint opportunities.
    if (documentFrames) {
      document.documentElement.getBoundingClientRect();
      await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    }
    window.ipc.postMessage(JSON.stringify({
      event: 'document_ready', document_frames: documentFrames, width: innerWidth, height: innerHeight,
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

const SCROLL_DOCUMENT: &str = r#"
function skriftBenchScrollDocument() {
  const contentHeight = () => Math.max(document.documentElement.scrollHeight, document.body.scrollHeight);
  const height = contentHeight();
  const viewportWidth = innerWidth;
  const viewportHeight = innerHeight;
  const scale = devicePixelRatio;
  const bottom = Math.max(0, height - viewportHeight);
  if (!Number.isFinite(height) || height <= 0 || viewportHeight <= 0)
    throw new Error('invalid simulated scroll geometry');
  scrollTo(0, 0);
  let offset = scrollY;
  if (Math.abs(offset) > 1) throw new Error('simulated scroll must start at the top');
  let viewports = 0;
  let remainingCallbacks = 2;
  function advance() {
    try {
      if (--remainingCallbacks > 0) {
        requestAnimationFrame(advance);
        return;
      }
      viewports++;
      if (offset + viewportHeight >= height - 1) {
        if (Math.abs(contentHeight() - height) > 1 || innerWidth !== viewportWidth
            || innerHeight !== viewportHeight || devicePixelRatio !== scale)
          throw new Error('geometry changed during simulated scrolling');
        window.ipc.postMessage(JSON.stringify({
          event:'scroll_complete',width:viewportWidth,height:viewportHeight,scale,
          layout_width:document.documentElement.clientWidth,content_width:document.documentElement.scrollWidth,
          content_height:height,scroll_y:scrollY,compatibility:document.compatMode,
          scroll:{viewports,start_y:0,step_px:viewportHeight,frame_callbacks_per_viewport:2}
        }));
        return;
      }
      const requested = Math.min(offset + viewportHeight, bottom);
      scrollTo(0, requested);
      const actual = scrollY;
      if (!Number.isFinite(actual) || actual <= offset || Math.abs(actual - requested) > 1)
        throw new Error('simulated scroll did not reach the next viewport');
      offset = actual;
      remainingCallbacks = 2;
      requestAnimationFrame(advance);
    } catch (error) {
      window.ipc.postMessage(JSON.stringify({event:'error',message:String(error)}));
    }
  }
  requestAnimationFrame(advance);
}
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
    timings: Rc<RefCell<Option<PhaseTimings>>>,
    phase_timings: bool,
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
        let timings: Rc<RefCell<Option<PhaseTimings>>> = Rc::default();
        let callback_timings = timings.clone();
        let callback_options = options.clone();
        let resource_root = options
            .fixture
            .parent()
            .context("fixture has no parent")?
            .to_owned();
        let ready_script = format!("{SCROLL_DOCUMENT}\n{READY}");
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
            .with_initialization_script_for_main_only(&ready_script, true)
            .with_ipc_handler(move |request| {
                let result = (|| -> Result<()> {
                    let metadata: Value = serde_json::from_str(request.body())?;
                    ensure!(metadata["event"] != "error", "page readiness failed: {}", metadata["message"]);
                    if callback_pending.replace(true) { return Ok(()); }
                    ensure!(metadata["width"].as_u64() == Some(callback_options.width.into())
                        && metadata["height"].as_u64() == Some(callback_options.height.into()),
                        "Wry child viewport mismatch: {metadata}");
                    let began: Instant = callback_began.get().context("page ready before document load")?;
                    let phase = callback_phase.get();
                    let mut timings = callback_timings.borrow_mut().take();
                    if callback_options.document_frames && matches!(phase, "warm" | "document") {
                        ensure!(metadata["document_frames"] == true,
                            "document did not run its frame callbacks");
                        if let Some(timings) = timings.as_mut() {
                            timings.mark("document_and_frames")?;
                        }
                        let elapsed = began.elapsed().as_secs_f64() * 1000.0;
                        emit(json!({"event":"ready", "phase":phase, "engine":"wry-in-gpui",
                            "document_frames":true, "document_to_render_ms":elapsed,
                            "frame_width":(metadata["width"].as_f64().context("viewport width")?
                                * metadata["scale"].as_f64().context("viewport scale")?).round() as u32,
                            "frame_height":(metadata["height"].as_f64().context("viewport height")?
                                * metadata["scale"].as_f64().context("viewport scale")?).round() as u32,
                            "frame_callbacks":2, "readback":false, "gpu_completion_fenced":false,
                            "page":metadata, "phase_timings":timings.and_then(PhaseTimings::into_value),
                            "endpoint":"document-sized viewport after two frame callbacks; no snapshot/readback; GPU completion is not fenced"}));
                        return Ok(());
                    }
                    if metadata["event"] == "scroll_complete" {
                        ensure!(callback_options.simulate_scroll && phase == "document",
                            "unexpected simulated scroll completion");
                        let mut timings = timings.context("missing simulated scroll timings")?;
                        // There is no host callback at page readiness now that
                        // the page starts scrolling without a round trip.
                        timings.mark("document_and_scroll")?;
                        let elapsed = timings.began.elapsed().as_secs_f64() * 1000.0;
                        emit(json!({"event":"ready", "phase":phase, "engine":"wry-in-gpui",
                            "simulated_scroll":true, "document_to_render_ms":elapsed,
                            "frame_width":(metadata["width"].as_f64().context("viewport width")?
                                * metadata["scale"].as_f64().context("viewport scale")?).round() as u32,
                            "frame_height":(metadata["height"].as_f64().context("viewport height")?
                                * metadata["scale"].as_f64().context("viewport scale")?).round() as u32,
                            "scroll":metadata["scroll"], "page":metadata,
                            "phase_timings":timings.into_value(),
                            "endpoint":"internal viewport scroll traversal after frame callbacks; no snapshot/readback; GPU completion is not fenced"}));
                        return Ok(());
                    }
                    ensure!(!(callback_options.simulate_scroll && phase == "document"),
                        "simulated scroll page did not run its traversal");
                    if let Some(timings) = timings.as_mut() {
                        timings.mark(if phase == "tile" { "scroll_ready" } else { "document_ready" })?;
                    }
                    let native = callback_native.upgrade().context("missing native WebView")?;
                    let screenshot = callback_options.screenshot.clone();
                    let document_surface = callback_options.document_surface && phase == "document";
                    native.snapshot(SnapshotRegion::Visible, SnapshotOptions::NONE,
                        None::<&gtk::gio::Cancellable>, move |snapshot| {
                        let snapshot_at = Instant::now();
                        let result = (|| -> Result<Value> {
                            let surface = ImageSurface::try_from(snapshot?)
                                .map_err(|_| anyhow::anyhow!("snapshot is not an image surface"))?;
                            if let Some(timings) = timings.as_mut() {
                                timings.mark("snapshot")?;
                            }
                            let dimensions = (surface.width(), surface.height());
                            if let Some(path) = screenshot { surface.write_to_png(&mut File::create(path)?)?; }
                            Ok(json!({"event":"ready", "phase":phase, "engine":"wry-in-gpui",
                                "document_surface":document_surface,
                                "pid":std::process::id(), "webview_version":wry::webview_version()?,
                                "parent_x11_window":parent_xid, "embedded_as_child":true,
                                "document_to_snapshot_ms":(snapshot_at-began).as_secs_f64()*1000.0,
                                "phase_timings":timings.and_then(PhaseTimings::into_value),
                                "snapshot_width":dimensions.0, "snapshot_height":dimensions.1,
                                "page":metadata,
                                "endpoint":if document_surface { "single document-sized WebKit surface snapshot complete; includes readback; excludes PNG encoding" }
                                    else { "WebKit child viewport snapshot complete; includes readback; excludes PNG encoding" }}))
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
        if options.document_surface || options.document_frames {
            // Explicitly allocate GTK's child after realization as well as the
            // X11 container; initial GTK requisitions can change tall bounds.
            webview.set_bounds(wry::Rect {
                position: wry::dpi::LogicalPosition::new(0, 0).into(),
                size: wry::dpi::LogicalSize::new(options.width, options.height).into(),
            })?;
        }
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
            timings,
            phase_timings: options.phase_timings,
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
        timings: PhaseTimings,
    ) -> Result<()> {
        self.began.set(Some(timings.began));
        self.timings.replace(Some(timings));
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
        if options.simulate_scroll && phase == "document" {
            base.query_pairs_mut().append_pair("skrift-bench-scroll", "1");
        }
        if options.document_frames {
            base.query_pairs_mut().append_pair("skrift-bench-frames", "1");
        }
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

    pub fn tile(&self, y: f64) -> Result<()> {
        let timings = PhaseTimings::new(self.phase_timings)?;
        self.began.set(Some(timings.began));
        self.timings.replace(Some(timings));
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
        Ok(())
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
