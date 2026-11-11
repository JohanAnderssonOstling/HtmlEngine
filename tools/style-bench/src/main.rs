//! Styling-only benchmark: parse and compile outside the measured interval.
//! Run with `cargo run --release --manifest-path tools/style-bench/Cargo.toml -- 31`.
//! Three warmups precede measurement. Allocation counts include reallocations;
//! byte counts are cumulative requested bytes, not peak or retained memory.
//! Fingerprints cover color, font size, width, and left margin; they are not a
//! substitute for the correctness suite. Timings include allocator instrumentation.
use html_style::{
    AuthorStylesheetInput, MediaEnvironment, StyleProgram, StyleTimings,
    resolve_document_with_program};
use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

struct Allocator;
static TRACK: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

fn record(bytes: usize) {
    if TRACK.load(Relaxed) {
        ALLOCATIONS.fetch_add(1, Relaxed);
        BYTES.fetch_add(bytes, Relaxed);
    }
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        unsafe { System.realloc(pointer, layout, size) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

fn workload(case: &str) -> (String, String) {
    let mut html = String::from("<!doctype html><html><body>");
    let mut css = String::from("body{color:black} p{display:block}");
    if case != "plain" {
        css.push_str("body{");
        for name in 0..128 {
            css.push_str(&format!("--v{name}:{}px;", name + 1));
        }
        css.push_str("--color:green} p{color:var(--color);width:calc(var(--v0) + var(--v1));margin-left:var(--local,0px)}");
    }
    match case {
        "deep" => {
            for branch in 0..24 {
                for depth in 0..96 {
                    html.push_str(&format!(
                        "<div style='--v{}:{}px;--local:var(--v{})'><p>text</p>",
                        depth % 128,
                        branch + depth + 1,
                        (depth + 1) % 128
                    ));
                }
                html.push_str(&"</div>".repeat(96));
            }
        }
        "substitution" => {
            css.push_str("p{--a:var(--v1);--b:calc(var(--a) + var(--v2));--c:var(--missing,var(--b));width:calc(var(--c) + var(--a));padding:var(--a) var(--b)}");
            for index in 0..2304 {
                html.push_str(&format!(
                    "<p style='--v1:{}px;--color:rgb(0 128 0)'>text</p>",
                    index % 97 + 1
                ));
            }
        }
        "overrides" => {
            for index in 0..2304 {
                html.push_str(&format!("<p style='--local:{}px'>text</p>", index % 97));
            }
        }
        "plain" => html.push_str(&"<p>text</p>".repeat(2304)),
        _ => panic!("unknown workload {case}"),
    }
    html.push_str("</body></html>");
    (html, css)
}

fn main() {
    let samples: usize = std::env::args()
        .nth(1)
        .map_or(21, |value| value.parse().expect("sample count"));
    assert!(samples > 0);
    println!("case,samples,median_ms,allocations,allocated_bytes,fingerprint");
    for case in ["plain", "overrides", "substitution", "deep"] {
        let (html, css) = workload(case);
        let document = html_parse::parse_dom_document(&html).expect("benchmark DOM");
        let inputs = [AuthorStylesheetInput {
            css: &css,
            implicit_scope_root: None,
        }];
        let author_roots = inputs.iter().map(|input| input.implicit_scope_root).collect::<Vec<_>>();
        let program = StyleProgram::compile(&[&css], MediaEnvironment::default(), 16.0);
        let mut times = Vec::with_capacity(samples);
        let mut allocations = Vec::with_capacity(samples);
        let mut bytes = Vec::with_capacity(samples);
        let mut fingerprint = None;
        for iteration in 0..samples + 3 {
            ALLOCATIONS.store(0, Relaxed);
            BYTES.store(0, Relaxed);
            TRACK.store(true, Relaxed);
            let started = Instant::now();
            let (styles, _, _) = resolve_document_with_program(
                &document,
                &program,
                &author_roots,
                StyleTimings::default(),
                &Default::default(),
            );
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            TRACK.store(false, Relaxed);
            if iteration >= 3 {
                times.push(elapsed);
                allocations.push(ALLOCATIONS.load(Relaxed));
                bytes.push(BYTES.load(Relaxed));
            }
            // Compare observable computed values, not store IDs or addresses.
            let mut hash = 0xcbf29ce484222325u64;
            for node in document.node_ids() {
                if let Some(indices) = styles.style_for_node(node) {
                    let text = styles.text_style(indices).unwrap();
                    let font = styles.font_style(indices).unwrap();
                    let layout = styles.box_model_style(indices).unwrap();
                    for byte in format!(
                        "{:?}:{:?}:{:?}:{:?}",
                        text.color, font.font_size, layout.width, layout.margin_left
                    )
                    .bytes()
                    {
                        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
                    }
                }
            }
            assert_eq!(*fingerprint.get_or_insert(hash), hash);
            black_box(&styles);
        }
        times.sort_by(f64::total_cmp);
        allocations.sort_unstable();
        bytes.sort_unstable();
        println!(
            "{case},{samples},{:.3},{},{},{:016x}",
            times[samples / 2],
            allocations[samples / 2],
            bytes[samples / 2],
            fingerprint.unwrap()
        );
    }
}
