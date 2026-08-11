//! Reproducible micro-benchmark for the style resolver's cascade stage.
//!
//! Run with:
//! `cargo run -p html-style --release --example cascade_benchmark -- 1500 25 48 8`
//!
//! Arguments are element count, measured iterations, overriding rule count,
//! the interval at which generated rules contain an important declaration,
//! whether winning declarations use `var()` (`1` or `0`), and the interval
//! between inline style attributes. Use zero for the important interval to
//! generate no important declarations, or zero for the inline interval to
//! generate no inline styles.

use html_style::style_document_with_timings;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::Duration;

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

fn workload(
    nodes: usize,
    overriding_rules: usize,
    important_every: usize,
    use_vars: bool,
    inline_every: usize,
    unique_inline: bool,
    inherited_custom_properties: usize,
) -> (String, String) {
    let mut html = String::from("<!doctype html><html><body>");
    for index in 0..nodes {
        let has_inline = inline_every > 0 && index % inline_every == 0;
        let inline = if has_inline && unique_inline {
            let padding = if use_vars { "var(--space)" } else { "3px" };
            format!(" style=\"width:{}px; padding-left:{padding}; color:revert-layer; font-feature-settings:'kern' {}\"", index + 1, index + 1)
        } else if has_inline && use_vars {
            " style='width:321px; padding-left:var(--space); color:revert-layer'".to_owned()
        } else if has_inline {
            " style='width:321px; padding-left:3px; color:revert-layer'".to_owned()
        } else {
            String::new()
        };
        html.push_str(&format!(
            "<div class='item group{}' data-index='{index}'{inline}>text</div>",
            index % 8
        ));
    }
    html.push_str("</body></html>");

    let mut css = String::from("@layer base, components, theme;\n");
    if inherited_custom_properties > 0 {
        css.push_str("body {");
        for property in 0..inherited_custom_properties {
            css.push_str(&format!("--inherited-{property}:{}px;", property + 1));
        }
        css.push_str("--space:3px;}\n");
    }
    let base_space = if inherited_custom_properties == 0 {
        "--space:3px;"
    } else {
        ""
    };
    css.push_str(&format!("@layer base {{ .item {{ {base_space} display:block; margin:1px; padding:2px; border:1px solid black; color:#123456; font-size:14px; line-height:1.4; width:100px; height:20px }} }}\n"));
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
        let local_space = if inherited_custom_properties == 0 {
            format!("--space:{}px;", rule % 9 + 1)
        } else {
            String::new()
        };
        css.push_str(&format!("@layer {layer} {{ .item {{ {local_space} margin:{}px; padding:{}px; border-width:{}px; color:rgb({} 20 30){important}; font-size:{}px; line-height:1.5; width:{}px; height:{}px; min-width:{}px; max-width:{}px; flex:{} 1 auto; background-color:#{:06x}; }} }}\n", rule % 7, rule % 5, rule % 4 + 1, rule % 255, rule % 6 + 12, rule + 100, rule % 20 + 20, rule % 15, rule + 300, rule % 4 + 1, rule * 123_457 % 0x00ff_ffff));
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
    let inline_every = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(10);
    let unique_inline = arguments.next().is_some_and(|value| value != "0");
    let inherited_custom_properties = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    assert!(nodes > 0 && iterations > 0);
    let (html, css) = workload(
        nodes,
        overriding_rules,
        important_every,
        use_vars,
        inline_every,
        unique_inline,
        inherited_custom_properties,
    );

    for _ in 0..3 {
        let document = html_parse::parse_dom_document(&html).expect("benchmark HTML parses");
        black_box(style_document_with_timings(document, &[&css]));
    }

    let mut cascade = Vec::with_capacity(iterations);
    let mut resolve = Vec::with_capacity(iterations);
    let mut measured_allocations = 0;
    let mut measured_peak_bytes = 0;
    for iteration in 0..iterations {
        let document = html_parse::parse_dom_document(&html).expect("benchmark HTML parses");
        if iteration == 0 {
            ALLOCATIONS.with(|count| count.set(Some(0)));
            ALLOCATION_BYTES.with(|bytes| bytes.set(Some((0, 0))));
        }
        let (styled, timings) = style_document_with_timings(document, &[&css]);
        if iteration == 0 {
            measured_allocations =
                ALLOCATIONS.with(|count| count.take().expect("allocation counter enabled"));
            measured_peak_bytes = ALLOCATION_BYTES
                .with(|bytes| bytes.take().expect("byte tracking enabled").1);
        }
        black_box(styled);
        cascade.push(timings.cascade);
        resolve.push(timings.resolve_styles);
    }
    cascade.sort_unstable();
    resolve.sort_unstable();

    println!(
        "nodes={nodes} iterations={iterations} overriding_rules={overriding_rules} important_every={important_every} use_vars={use_vars} inline_every={inline_every} unique_inline={unique_inline} inherited_custom_properties={inherited_custom_properties}"
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
    println!("style_pipeline_peak_bytes={measured_peak_bytes}");
}
