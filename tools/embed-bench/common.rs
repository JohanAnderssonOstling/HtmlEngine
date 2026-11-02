use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone)]
pub struct Options {
    pub fixture: PathBuf,
    pub width: u32,
    pub height: u32,
    pub screenshot: Option<PathBuf>,
    pub startup_document: bool,
    pub native_window: bool,
    pub content: String,
    pub phase_timings: bool,
    pub simulate_scroll: bool,
    pub document_surface: bool,
    pub document_frames: bool,
}

impl Options {
    pub fn parse() -> Result<Self> {
        let mut args = std::env::args().skip(1);
        let mut fixture = None;
        let mut width = 800;
        let mut height = 600;
        let mut screenshot = None;
        let mut startup_document = false;
        let mut native_window = false;
        let mut content = "skrift".to_owned();
        let mut phase_timings = false;
        let mut simulate_scroll = false;
        let mut document_surface = false;
        let mut document_frames = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--fixture" => {
                    fixture = Some(PathBuf::from(args.next().context("missing fixture")?))
                }
                "--width" => width = args.next().context("missing width")?.parse()?,
                "--height" => height = args.next().context("missing height")?.parse()?,
                "--startup-document" => startup_document = true,
                "--native-window" => native_window = true,
                "--content" => content = args.next().context("missing content engine")?,
                "--phase-timings" => phase_timings = true,
                "--simulate-scroll" => simulate_scroll = true,
                "--document-surface" => document_surface = true,
                "--document-frames" => document_frames = true,
                "--screenshot" => {
                    screenshot = Some(PathBuf::from(
                        args.next().context("missing screenshot path")?,
                    ))
                }
                "--help" => {
                    println!(
                        "--fixture PATH [--width 800] [--height 600] [--screenshot PATH] [--startup-document | --native-window]\nJSON events on stdout; send 'load' after blank/native readiness, or 'quit'. Native mode defers HTML content initialization. See ../run.py --help."
                    );
                    std::process::exit(0);
                }
                _ => bail!("unknown argument: {arg}"),
            }
        }
        let limit = if document_surface || document_frames { 16384 } else { 8192 };
        if width == 0 || height == 0 || width > limit || height > limit {
            bail!("viewport dimensions must be between 1 and {limit}");
        }
        if startup_document && native_window {
            bail!("--startup-document and --native-window are mutually exclusive");
        }
        if simulate_scroll && (!native_window || startup_document || screenshot.is_some()) {
            bail!(
                "--simulate-scroll requires --native-window and excludes startup/screenshot capture"
            );
        }
        if document_surface
            && (!native_window || startup_document || screenshot.is_some() || simulate_scroll || document_frames)
        {
            bail!(
                "--document-surface requires --native-window and excludes startup, scrolling and measured PNG capture"
            );
        }
        if document_frames
            && (!native_window || startup_document || screenshot.is_some() || simulate_scroll)
        {
            bail!("--document-frames requires --native-window and excludes startup, scrolling and measured PNG capture");
        }
        if !matches!(content.as_str(), "skrift" | "wry") {
            bail!("--content must be skrift or wry");
        }
        Ok(Self {
            fixture: fixture.context("--fixture is required")?.canonicalize()?,
            width,
            height,
            screenshot,
            startup_document,
            native_window,
            content,
            phase_timings,
            simulate_scroll,
            document_surface,
            document_frames,
        })
    }
}

/// Sequential host intervals. Cgroup CPU includes every host/helper thread,
/// including unrelated work that runs concurrently with a measured phase.
pub struct PhaseTimings {
    pub began: Instant,
    enabled: bool,
    previous: Instant,
    cpu_stat: Option<PathBuf>,
    previous_cpu: Option<u64>,
    phases: Map<String, Value>,
}

impl PhaseTimings {
    pub fn new(enabled: bool) -> Result<Self> {
        let cpu_stat = enabled
            .then(|| std::env::var_os("SKRIFT_EMBED_CPU_STAT").map(PathBuf::from))
            .flatten();
        let previous_cpu = Self::read_cpu(cpu_stat.as_ref())?;
        let began = Instant::now();
        Ok(Self {
            began,
            enabled,
            previous: began,
            cpu_stat,
            previous_cpu,
            phases: Map::new(),
        })
    }

    fn read_cpu(path: Option<&PathBuf>) -> Result<Option<u64>> {
        let Some(path) = path else { return Ok(None) };
        let stat = fs::read_to_string(path).context("read phase cgroup CPU counter")?;
        let usage = stat
            .lines()
            .find_map(|line| line.strip_prefix("usage_usec "))
            .context("phase cgroup CPU counter is missing usage_usec")?
            .parse()?;
        Ok(Some(usage))
    }

    pub fn mark(&mut self, name: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let cpu = Self::read_cpu(self.cpu_stat.as_ref())?;
        let now = Instant::now();
        let mut phase = json!({"wall_ms": (now - self.previous).as_secs_f64() * 1000.0});
        if let (Some(before), Some(after)) = (self.previous_cpu, cpu) {
            phase["cpu_ms"] = json!(
                after
                    .checked_sub(before)
                    .context("phase cgroup CPU counter decreased")? as f64
                    / 1000.0
            );
        }
        self.phases.insert(name.to_owned(), phase);
        self.previous = now;
        self.previous_cpu = cpu;
        Ok(())
    }

    pub fn into_value(self) -> Option<Value> {
        self.enabled.then_some(Value::Object(self.phases))
    }
}

pub fn emit(value: Value) {
    let mut out = io::stdout().lock();
    writeln!(out, "{value}").expect("write benchmark event");
    out.flush().expect("flush benchmark event");
}

pub fn blank_html() -> &'static str {
    "<!doctype html><html><head><style>html,body{margin:0;background:white}</style></head><body></body></html>"
}
