//! Reproducible micro-benchmark for the style resolver's cascade stage.
//!
//! Run with:
//! `cargo run -p html-style --release --example cascade_benchmark -- 1500 25 48 8`
//!
//! Arguments are element count, measured iterations, overriding rule count,
//! the interval at which generated rules contain an important declaration,
//! and whether winning declarations use `var()` (`1` or `0`). Use zero for
//! the important interval to generate no important declarations.

use html_style::style_document_with_timings;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::Duration;

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

struct TrackingAllocator;

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get().map(|value| value + 1)));
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get().map(|value| value + 1)));
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get().map(|value| value + 1)));
        unsafe { System.realloc(pointer, layout, new_size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

fn workload(
    nodes: usize,
    overriding_rules: usize,
    important_every: usize,
    use_vars: bool,
) -> (String, String) {
    let mut html = String::from("<!doctype html><html><body>");
    for index in 0..nodes {
        let inline = if index % 10 == 0 && use_vars {
            " style='width:321px; padding-left:var(--space); color:revert-layer'"
        } else if index % 10 == 0 {
            " style='width:321px; padding-left:3px; color:revert-layer'"
        } else {
            ""
        };
        html.push_str(&format!(
            "<div class='item group{}' data-index='{index}'{inline}>text</div>",
            index % 8
        ));
    }
    html.push_str("</body></html>");

    let mut css = String::from(
        "@layer base, components, theme;\n@layer base { .item { --space:3px; display:block; margin:1px; padding:2px; border:1px solid black; color:#123456; font-size:14px; line-height:1.4; width:100px; height:20px } }\n",
    );
    for rule in 0..overriding_rules {
        let layer = match rule % 3 {
            0 => "base",
            1 => "components",
            _ => "theme",
        };
        let important = if important_every > 0 && rule % important_every == 0 {
            " !important"
        } else {
            ""
        };
        css.push_str(&format!("@layer {layer} {{ .item {{ --space:{}px; margin:{}px; padding:{}px; border-width:{}px; color:rgb({} 20 30){important}; font-size:{}px; line-height:1.5; width:{}px; height:{}px; min-width:{}px; max-width:{}px; flex:{} 1 auto; background-color:#{:06x}; }} }}\n", rule % 9 + 1, rule % 7, rule % 5, rule % 4 + 1, rule % 255, rule % 6 + 12, rule + 100, rule % 20 + 20, rule % 15, rule + 300, rule % 4 + 1, rule * 123_457 % 0x00ff_ffff));
    }
    for group in 0..8 {
        let padding = if use_vars { "var(--space)" } else { "3px" };
        css.push_str(&format!(".group{group} {{ width:{}px; padding-left:{padding}; background-color:rgb({} 40 50); }}\n", 200 + group, group * 20));
    }
    css.push_str(
        ".item { width:777px; width:-1px; border-left-width:5px; border-left-width:-2px; }\n",
    );
    (html, css)
}

fn percentile(samples: &[Duration], numerator: usize, denominator: usize) -> Duration {
    samples[(samples.len() - 1) * numerator / denominator]
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let nodes = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1500);
    let iterations = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(25);
    let overriding_rules = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(48);
    let important_every = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let use_vars = arguments.next().is_none_or(|value| value != "0");
    assert!(nodes > 0 && iterations > 0);
    let (html, css) = workload(nodes, overriding_rules, important_every, use_vars);

    for _ in 0..3 {
        let document = html_parse::parse_dom_document(&html).expect("benchmark HTML parses");
        black_box(style_document_with_timings(document, &[&css]));
    }

    let mut cascade = Vec::with_capacity(iterations);
    let mut resolve = Vec::with_capacity(iterations);
    let mut measured_allocations = 0;
    for iteration in 0..iterations {
        let document = html_parse::parse_dom_document(&html).expect("benchmark HTML parses");
        if iteration == 0 {
            ALLOCATIONS.with(|count| count.set(Some(0)));
        }
        let (styled, timings) = style_document_with_timings(document, &[&css]);
        if iteration == 0 {
            measured_allocations =
                ALLOCATIONS.with(|count| count.take().expect("allocation counter enabled"));
        }
        black_box(styled);
        cascade.push(timings.cascade);
        resolve.push(timings.resolve_styles);
    }
    cascade.sort_unstable();
    resolve.sort_unstable();

    println!(
        "nodes={nodes} iterations={iterations} overriding_rules={overriding_rules} important_every={important_every} use_vars={use_vars}"
    );
    println!(
        "cascade_median_us={}",
        percentile(&cascade, 1, 2).as_micros()
    );
    println!(
        "cascade_p95_us={}",
        percentile(&cascade, 95, 100).as_micros()
    );
    println!(
        "resolve_median_us={}",
        percentile(&resolve, 1, 2).as_micros()
    );
    println!(
        "resolve_p95_us={}",
        percentile(&resolve, 95, 100).as_micros()
    );
    println!("style_pipeline_allocations={measured_allocations}");
}
