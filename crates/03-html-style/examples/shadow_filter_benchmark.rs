//! Dense overlapping-rule stress benchmark for early cascade pruning.
//!
//! Run with: `cargo run -p skrift-style --release --example shadow_filter_benchmark`
//! Unique inline styles keep every element on the cascade path.

use html_style::style_document_with_timings;
use std::{hint::black_box, time::Duration};
fn main() {
    let mut html = String::from("<!doctype html><html><body>");
    for i in 0..1000 {
        html.push_str(&format!(
            "<p class='item' style='margin-top:{i}px'>text</p>"
        ));
    }
    html.push_str("</body></html>");
    let mut css = String::new();
    for i in 0..128 {
        css.push_str(&format!(
            ".item {{ color:rgb({i} 20 30); background-color:rgb(20 {i} 30); text-align:left }}"
        ));
    }
    let mut samples = Vec::<Duration>::new();
    for i in 0..24 {
        let doc = html_parse::parse_dom_document(&html).unwrap();
        let (styled, timings) = style_document_with_timings(doc, &[&css]);
        black_box(styled);
        if i >= 3 {
            samples.push(timings.cascade);
        }
    }
    samples.sort();
    println!(
        "dense_cascade_median_us={}",
        samples[samples.len() / 2].as_micros()
    );
}
