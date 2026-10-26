//! Document-query workloads. Parse, shaping, and layout are outside timing.
//! Run query_search, query_table, query_roles, and query_restore with the usual
//! fixture/count arguments. Table mode uses a fixed 500-row, eight-column table.

use crate::{Shaper, access::measure, layout_fingerprint};
use html::layout::{ContentRole, LayoutConstraints};
use html::pipeline::DocumentFactory;
use html::text::TextSearchOptions;
use serde_json::json;
use std::hint::black_box;

pub(super) fn run(path: &str, case: &str, count: usize) {
    let source = if case == "query_table" {
        let rows = (0..500)
            .map(|row| {
                format!(
                    "<tr>{}</tr>",
                    (0..8)
                        .map(|column| format!("<td>Cell {row}:{column}</td>"))
                        .collect::<String>()
                )
            })
            .collect::<String>();
        format!(
            "<table><thead><tr>{}</tr></thead><tbody>{rows}</tbody></table>",
            "<th>Header</th>".repeat(8)
        )
    } else {
        std::fs::read_to_string(path).unwrap()
    };
    let document = DocumentFactory::new()
        .parse_with_new_pipeline(&source, None)
        .shape(&mut Shaper::default())
        .unwrap()
        .layout(LayoutConstraints::new(600.0, 16.0).unwrap());
    let view = document.render_view();
    let mut observed = 0;
    let mut result = match case {
        "query_search" => measure(count, || {
            observed = black_box(view.text().find_matches(
                black_box("the"),
                TextSearchOptions {
                    whole_word: true,
                    ..Default::default()
                },
            ))
            .len();
        }),
        "query_roles" => measure(count, || {
            observed = view
                .text()
                .text_runs()
                .filter_map(|run| {
                    black_box(
                        view.semantics()
                            .nearest(black_box(run.box_idx()), ContentRole::ListItem),
                    )
                })
                .count();
        }),
        "query_restore" => {
            let locations = view
                .text()
                .text_runs()
                .step_by(100)
                .filter_map(|run| {
                    view.addressing()
                        .source_position_for_glyph(run.glyphs().start, false)
                })
                .collect::<Vec<_>>();
            assert!(!locations.is_empty());
            measure(count, || {
                observed = locations
                    .iter()
                    .filter_map(|source| {
                        let steps = source
                            .elements()
                            .map(|step| step.step())
                            .collect::<Vec<_>>();
                        black_box(view.addressing().resolve_source_position(
                            &steps,
                            Some(source.text_step()),
                            Some(source.utf16_offset()),
                            false,
                        ))
                    })
                    .count();
            })
        }
        "query_table" => {
            let table = (0..view.boxes().len())
                .find(|&index| view.boxes().is_table(index))
                .unwrap();
            measure(count, || {
                let table = view.boxes().table(black_box(table)).unwrap();
                let ranges = table
                    .rows()
                    .iter()
                    .flat_map(|row| row.cells())
                    .filter_map(|cell| cell.glyphs())
                    .collect::<Vec<_>>();
                let markdown = table.markdown_in(100..400, 2..6).unwrap();
                observed = black_box(ranges.len() + markdown.len());
            })
        }
        _ => panic!("unknown query workload {case}"),
    };
    result["case"] = json!(case);
    result["observed"] = json!(observed);
    result["characters"] = json!(view.text().glyph_count());
    result["lines"] = json!(view.text().line_count());
    result["integrity"] = json!(format!("{:016x}", layout_fingerprint(&document)));
    println!("{result}");
}
