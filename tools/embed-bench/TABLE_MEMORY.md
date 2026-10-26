# Table memory investigation

The 80 row fixture has four columns, a header row, collapsed borders and no images. The earlier native window comparison reported 12.45 MiB added PSS. That number describes additional resident process memory after first content and a viewport snapshot, rather than the size of the retained table document.

The refined diagnostic run measured **11.32 MiB added PSS**, with this mapping breakdown:

| Mapping category | Added PSS |
| --- | ---: |
| Heap pages | 6.38 MiB |
| Mapped font files | 3.12 MiB |
| Executable pages | 1.81 MiB |
| Other mappings | 0.00 MiB |

Heap pages include live allocations and freed storage retained by the allocator. Font and executable pages are first use residency costs; their PSS also depends on sharing with other processes. They are not all private table storage.

## Retained allocations

| Measure | Table | Article |
| --- | ---: | ---: |
| Engine document capacity estimate | 1.04 MiB | 0.09 MiB |
| Increase in live Rust requested allocations | 2.23 MiB | 1.02 MiB |

The engine estimate is included within live Rust allocations, which are included within process memory: these values must not be added together. Engine reports cover owned capacities and shared storage deduplication where supported, not exhaustive allocator attribution. The global Rust allocator counter excludes allocations made directly by C libraries and graphics drivers.

The largest engine categories for the table are:

| Category | Estimated allocated capacity |
| --- | ---: |
| layout_state | 372.1 KiB |
| document | 136.8 KiB |
| layout_tree | 106.2 KiB |
| paragraph_layouts | 83.9 KiB |
| text_geometry | 81.4 KiB |
| inline_plans | 80.5 KiB |
| glyph_metrics | 80.1 KiB |

The retained paint program alone has 176 KiB of command capacity. The adapter retains 247 shaped line cache entries and 326 backend glyph records for 2,556 registered logical glyph positions. The viewport paint command buffer has 22 KiB of capacity. There are no decoded image pixels for this fixture.

## Transient memory and clearing

The 800 × 600 RGBA snapshot owns exactly 1,920,000 bytes (1.83 MiB). The tracked live allocation counter decreases by exactly that amount when the image is dropped. The buffer is gone before settled PSS is measured, but freeing it does not guarantee its allocator pages return to the OS.

Calling `malloc_trim(0)` while the document remains loaded returned **1.77 MiB of PSS** with no material change in live Rust allocations or engine document capacity. This is a diagnostic experiment, not a change to the application allocator policy.

Clearing the document and trimming returns the engine estimate to about 9 KiB. Resetting the whole document view and adapter, redrawing blank and trimming leaves 4.63 MiB above the original baseline. Font mappings, touched executable pages and window text/raster state can remain available after the table is gone. The measured load/clear sequence does not show the table document being leaked, but it does not prove leak freedom over repeated or long running workloads.

## Main opportunities

1. Load only the font face needed for the requested weight/style. `CosmicTextSystemState::load_family` currently opens every face in the requested family and its configured fallback families. The uninstrumented validation observed additional resident pages in 25 font files despite this being a basic Latin table. This is a shared first content cost rather than an 80 row table requirement.
2. Inspect snapshot/readback buffer reuse and allocator retention before treating all added PSS as live engine data. A temporary 1.83 MiB image is demonstrably released, and allocator trimming can reclaim resident pages while the table stays loaded.
3. For engine storage specifically, paint/layout output is the largest group. Its roughly 372 KiB is a more focused target than trying to remove a supposed 12 MiB table allocation. Shape and paragraph caches are additional bounded storage to consider for document switching.

## Scope and reproducibility

The diagnostic executable wraps the system allocator to count requested live Rust bytes and reads glibc allocator statistics. It stops at pipeline boundaries while the controller reads Linux smaps, so no performance timing from this executable is used. The source tree is being worked on concurrently: the diagnostic binary is a fresh build, separate from the preserved measured host. Capacity estimates are not exact byte for byte attribution of the earlier run.

The saved original executable was also checked without hooks: table added PSS was 9.74 MiB and article added PSS was 9.73 MiB in those observations. The first instrumented table run was 12.34 MiB and the refined run was 11.32 MiB; retained engine storage stayed about 1.05 MiB and additional live Rust storage about 2.23 MiB in both table profiles. PSS depends on allocator state, renderer state and file page sharing. These are diagnostic observations, not a distribution or a performance improvement measurement.

```sh
cargo build --release --manifest-path tools/embed-bench/gpui/Cargo.toml \
  --bin embed-bench-gpui-memory --features memory-profile

FONTCONFIG_FILE="$PWD/tools/embed-bench/results/language-fonts-20261026/fonts.conf" \
python3 tools/embed-bench/investigate_memory.py \
  --binary tools/embed-bench/gpui/target/release/embed-bench-gpui-memory \
  --fixture tools/embed-bench/fixtures/tables.html \
  --output tools/embed-bench/results/tables-memory-new.json
```

- [Summary and source hashes](results/tables-memory-summary-20261027.json)
- [Refined table checkpoints and mappings](results/tables-memory-refined-20261027.json)
- [Article checkpoints](results/article-memory-stages-20261027.json)
- [First table profile](results/tables-memory-stages-20261027.json)
- [Saved executable mapping check](results/tables-memory-original-20261027.json)
