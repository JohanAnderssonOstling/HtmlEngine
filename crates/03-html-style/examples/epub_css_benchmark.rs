//! End-to-end style-pipeline benchmark over CSS extracted from real EPUBs.
//!
//! Run with:
//! `cargo run -p html-style --release --example epub_css_benchmark -- 15 4`
//!
//! Arguments are measured iterations and synthetic page copies.

use html_style::{StyleTimings, style_document_with_timings};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeSet;
use std::hint::black_box;
use std::time::{Duration, Instant};

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
    static ALLOCATION_BYTES: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
}

fn allocated(size: usize) {
    ALLOCATION_BYTES.with(|bytes| {
        if let Some((current, peak)) = bytes.get() {
            let current = current + size;
            bytes.set(Some((current, peak.max(current))));
        }
    });
}

fn deallocated(size: usize) {
    ALLOCATION_BYTES.with(|bytes| {
        if let Some((current, peak)) = bytes.get() {
            bytes.set(Some((current.saturating_sub(size), peak)));
        }
    });
}

struct TrackingAllocator;

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get().map(|value| value + 1)));
        allocated(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get().map(|value| value + 1)));
        allocated(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get().map(|value| value + 1)));
        deallocated(layout.size());
        allocated(new_size);
        unsafe { System.realloc(pointer, layout, new_size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        deallocated(layout.size());
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

struct Sample {
    name: &'static str,
    css: &'static [&'static str],
}

const SAMPLES: &[Sample] = &[
    Sample {
        name: "small-classic",
        css: &[include_str!("../bench-data/epub-css/01-small-classic.css")],
    },
    Sample {
        name: "trade-book",
        css: &[include_str!("../bench-data/epub-css/02-trade-book.css")],
    },
    Sample {
        name: "technical",
        css: &[
            include_str!("../bench-data/epub-css/03-technical.css"),
            include_str!("../bench-data/epub-css/03-technical-page.css"),
        ],
    },
    Sample {
        name: "visual",
        css: &[
            include_str!("../bench-data/epub-css/04-visual.css"),
            include_str!("../bench-data/epub-css/04-visual-page.css"),
        ],
    },
    Sample {
        name: "reference",
        css: &[include_str!("../bench-data/epub-css/05-reference.css")],
    },
];

#[derive(Default)]
struct Samples {
    html_parse: Vec<Duration>,
    total_style: Vec<Duration>,
    parse_author_css: Vec<Duration>,
    prepare_rules: Vec<Duration>,
    selector_index: Vec<Duration>,
    selector_matching: Vec<Duration>,
    cascade: Vec<Duration>,
}

impl Samples {
    fn push(&mut self, html_parse: Duration, total_style: Duration, timings: StyleTimings) {
        self.html_parse.push(html_parse);
        self.total_style.push(total_style);
        self.parse_author_css.push(timings.parse_author_css);
        self.prepare_rules.push(timings.prepare_rules);
        self.selector_index.push(timings.selector_index);
        self.selector_matching.push(timings.selector_matching);
        self.cascade.push(timings.cascade);
    }

    fn sort(&mut self) {
        self.html_parse.sort_unstable();
        self.total_style.sort_unstable();
        self.parse_author_css.sort_unstable();
        self.prepare_rules.sort_unstable();
        self.selector_index.sort_unstable();
        self.selector_matching.sort_unstable();
        self.cascade.sort_unstable();
    }
}

fn median(samples: &[Duration]) -> u128 {
    samples[samples.len() / 2].as_micros()
}

fn is_ident_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

fn class_names(css_chunks: &[&str]) -> Vec<String> {
    let mut names = BTreeSet::new();
    for css in css_chunks {
        let bytes = css.as_bytes();
        let mut cursor = 0;
        while cursor < bytes.len() {
            if bytes[cursor] != b'.'
                || cursor + 1 == bytes.len()
                || !is_ident_byte(bytes[cursor + 1])
                || bytes[cursor + 1].is_ascii_digit()
            {
                cursor += 1;
                continue;
            }
            let start = cursor + 1;
            cursor = start;
            while cursor < bytes.len() && is_ident_byte(bytes[cursor]) {
                cursor += 1;
            }
            if let Ok(name) = std::str::from_utf8(&bytes[start..cursor]) {
                names.insert(name.to_owned());
            }
        }
    }
    names.into_iter().collect()
}

fn workload(css_chunks: &[&str], page_copies: usize) -> (String, usize) {
    let classes = class_names(css_chunks);
    let mut html = String::from("<!doctype html><html><body class='calibre coverbody'>");
    let tags = [
        "div",
        "p",
        "span",
        "blockquote",
        "h1",
        "h2",
        "h3",
        "ol",
        "ul",
        "li",
        "table",
        "tr",
        "td",
    ];
    for page in 0..page_copies {
        html.push_str(&format!("<section id='page-{page}' class='chapter page'>"));
        for (index, class) in classes.iter().enumerate() {
            let tag = tags[index % tags.len()];
            let next = &classes[(index + 1) % classes.len()];
            html.push_str(&format!("<{tag} class='{class} {next}'>sample</{tag}>"));
        }
        html.push_str("</section>");
    }
    html.push_str("</body></html>");
    (html, classes.len())
}

fn run(sample: &Sample, iterations: usize, page_copies: usize) {
    let (html, class_count) = workload(sample.css, page_copies);
    for _ in 0..2 {
        let document = html_parse::parse_dom_document(&html).expect("benchmark HTML parses");
        black_box(style_document_with_timings(document, sample.css));
    }

    let mut samples = Samples::default();
    let mut style_allocations = 0;
    let mut style_peak_bytes = 0;
    let mut node_count = 0;
    for iteration in 0..iterations {
        let started = Instant::now();
        let document = html_parse::parse_dom_document(&html).expect("benchmark HTML parses");
        let html_parse = started.elapsed();
        node_count = document.node_count();

        if iteration == 0 {
            ALLOCATIONS.with(|count| count.set(Some(0)));
            ALLOCATION_BYTES.with(|bytes| bytes.set(Some((0, 0))));
        }
        let started = Instant::now();
        let (styled, timings) = style_document_with_timings(document, sample.css);
        let total_style = started.elapsed();
        if iteration == 0 {
            style_allocations =
                ALLOCATIONS.with(|count| count.take().expect("allocation counter enabled"));
            style_peak_bytes = ALLOCATION_BYTES
                .with(|bytes| bytes.take().expect("byte tracking enabled").1);
        }
        black_box(styled);
        samples.push(html_parse, total_style, timings);
    }
    samples.sort();

    let css_bytes = sample.css.iter().map(|css| css.len()).sum::<usize>();
    println!(
        "sample={} css_bytes={} classes={} nodes={} html_parse_us={} style_total_us={} css_parse_us={} prepare_us={} index_us={} matching_us={} cascade_us={} style_allocations={} style_peak_bytes={}",
        sample.name,
        css_bytes,
        class_count,
        node_count,
        median(&samples.html_parse),
        median(&samples.total_style),
        median(&samples.parse_author_css),
        median(&samples.prepare_rules),
        median(&samples.selector_index),
        median(&samples.selector_matching),
        median(&samples.cascade),
        style_allocations,
        style_peak_bytes,
    );
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let iterations = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(15);
    let page_copies = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4);
    assert!(iterations > 0 && page_copies > 0);

    println!("iterations={iterations} page_copies={page_copies}");
    for sample in SAMPLES {
        run(sample, iterations, page_copies);
    }
}
