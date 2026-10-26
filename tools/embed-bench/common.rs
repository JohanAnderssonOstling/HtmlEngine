use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::io::{self, Write};
use std::path::PathBuf;

#[derive(Clone)]
pub struct Options {
    pub fixture: PathBuf,
    pub width: u32,
    pub height: u32,
    pub screenshot: Option<PathBuf>,
}

impl Options {
    pub fn parse() -> Result<Self> {
        let mut args = std::env::args().skip(1);
        let mut fixture = None;
        let mut width = 800;
        let mut height = 600;
        let mut screenshot = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--fixture" => {
                    fixture = Some(PathBuf::from(args.next().context("missing fixture")?))
                }
                "--width" => width = args.next().context("missing width")?.parse()?,
                "--height" => height = args.next().context("missing height")?.parse()?,
                "--screenshot" => {
                    screenshot = Some(PathBuf::from(
                        args.next().context("missing screenshot path")?,
                    ))
                }
                "--help" => {
                    println!(
                        "--fixture PATH [--width 800] [--height 600] [--screenshot PATH]\nJSON events on stdout; send 'load' after blank readiness, or 'quit'. See ../run.py --help."
                    );
                    std::process::exit(0);
                }
                _ => bail!("unknown argument: {arg}"),
            }
        }
        if width == 0 || height == 0 || width > 8192 || height > 8192 {
            bail!("viewport dimensions must be between 1 and 8192");
        }
        Ok(Self {
            fixture: fixture.context("--fixture is required")?.canonicalize()?,
            width,
            height,
            screenshot,
        })
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
