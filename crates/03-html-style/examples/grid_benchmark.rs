//! Grid conversion benchmark; pass `retry` to exercise an invalid later template.
use html_style::style_document_with_timings;
use std::{hint::black_box, time::Duration};

fn main() {
    let retry = std::env::args().nth(1).as_deref() == Some("retry");
    let invalid = if retry {
        ";grid-template-columns:repeat(auto-fit,10px) repeat(auto-fill,20px)"
    } else {
        ""
    };
    let mut html = String::from("<!doctype html><html><body>");
    for i in 0..1500 {
        html.push_str(&format!(
            "<div style='display:grid;width:400px;margin-top:{i}px;\
             grid-template-columns:[a] minmax(10px,1fr) \
             repeat(2,[b] minmax(auto,20px)) repeat(auto-fit,[c] minmax(5px,1fr));\
             grid-template-rows:repeat(4,10px 20px){invalid}'>text</div>"
        ));
    }
    html.push_str("</body></html>");
    let mut samples = Vec::<Duration>::new();
    for i in 0..24 {
        let doc = html_parse::parse_dom_document(&html).unwrap();
        let (styled, timings) = style_document_with_timings(doc, &[]);
        black_box(styled);
        if i >= 3 {
            samples.push(timings.cascade);
        }
    }
    samples.sort();
    println!(
        "grid_cascade_median_us={}",
        samples[samples.len() / 2].as_micros()
    );
}
