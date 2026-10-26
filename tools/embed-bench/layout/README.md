# Comparison through layout

This is a separate benchmark from the native window comparison in
[../RESULTS.md](../RESULTS.md). It does not update or replace those results.
Each host starts with an empty document, then processes one fixture once.

Skrift runs its engine directly with the same GPUI/Cosmic text shaping backend
used by the window benchmark. `WindowTextSystem` is a text cache here: no window,
GPU device, sprite atlas, scene, paint commands or screenshot is created.
Images are decoded for their intrinsic dimensions and fed back into layout.

Chromium uses **chrome-headless-shell**, with manual BeginFrame control enabled
for the target. No frames or screenshots are requested. Ordinary Chromium
headless mode does not guarantee this behavior. The runner records traces and
rejects a sample if it observes painting, raster tasks or frame drawing. SVG
prepaint bookkeeping can still occur; it does not paint pixels.

The default viewport is 800 × 600 at scale 1. Both hosts run without `DISPLAY`
or `WAYLAND_DISPLAY`. Fixtures use the installed DejaVu Sans font family and
inline resources. Additional Arabic, Hebrew, Chinese, Japanese and Korean
fixtures are in `../fixtures/languages/`.

- **Engine CPU time (primary comparison):** Skrift calling-thread CPU time
  inside parse/style preparation, shaping and layout calls, including relayout
  after image dimensions arrive. Chromium thread CPU intervals from traced
  `ParseHTML`, `ParseAuthorStyleSheet`, `UpdateLayoutTree` and `Layout` events.
  Overlapping intervals per thread are counted once. Waiting is excluded;
  helper thread and process work outside these intervals is not included.
- **Engine elapsed time (secondary):** wall time inside the corresponding
  calls or traced stages. Nested intervals are counted once. This includes
  waiting and scheduling delays within those intervals.
- **Load to layout time:** a separate end-to-end diagnostic. Skrift includes file
  reading and result preparation; Chromium includes DevTools transport, resource
  readiness and geometry inspection. These have different control overheads.
- **Added memory:** settled process-session PSS after loading minus the settled
  empty-document PSS. Helper processes are included, shared pages proportionally.
  Trace collection is finished before reading memory; retained instrumentation
  allocations can still contribute. It is not a peak memory measurement.

Engine timings exclude separately scheduled image preparation and gaps between
stages. The boundaries and instrumentation differ between engines, so this is
an initial comparison rather than a precisely matched profiler experiment.
A fresh host is used for each fixture. The empty document is initialized, but
there are no warmup document loads. First use of document fonts is included.
The runner records one sample, not a distribution.

Build the additional executable using the existing sibling checkouts:

```sh
cargo build --release --manifest-path tools/embed-bench/gpui/Cargo.toml \
  --bin embed-bench-layout
```

Run it with an installed Chromium headless shell and Python `websocket-client`:

```sh
python3 tools/embed-bench/layout/run.py \
  --skrift tools/embed-bench/gpui/target/release/embed-bench-layout \
  --chromium /path/to/chrome-headless-shell \
  --output tools/embed-bench/results/layout-new.json
```

If `--chromium` is omitted, the runner finds a cached Playwright headless shell.
Existing report files and trace directories are never overwritten. Raw browser
traces are saved alongside each new report. The main native window benchmark
runner, its binaries and previous reports remain available.

For interpretation, both engines completing layout does not establish identical
output. Image dimensions and rectangles are checked without painting. The earlier
mixed RTL placement mismatch remains a correctness concern; no new pixel
comparison is claimed by this benchmark.

The initial observations are in [RESULTS.md](RESULTS.md).
The latest language rerun using CPU time as the main comparison is in
[CPU_RESULTS.md](CPU_RESULTS.md).
The later [RTL width correction](RTL_FIX.md) fixes incorrect native character
advances; the older saved RTL performance samples retain the earlier behavior.

Language coverage and its separate results are in
[LANGUAGE_RESULTS.md](LANGUAGE_RESULTS.md). To reproduce the shared CJK font
environment, prepare a private copy of the application's bundled font using
Python `fontTools`:

```sh
python3 tools/embed-bench/layout/prepare_language_fonts.py \
  --source ../Html-Prototype/apps/desktop-gpui/assets/fonts/DroidSansFallback.ttf \
  --license ../Html-Prototype/apps/desktop-gpui/assets/fonts/LICENSE-DROID.txt \
  --output tools/embed-bench/results/language-fonts-new

python3 tools/embed-bench/layout/run.py \
  --skrift tools/embed-bench/gpui/target/release/embed-bench-layout \
  --fontconfig tools/embed-bench/results/language-fonts-new/fonts.conf \
  --inspect-fonts \
  --fixtures tools/embed-bench/fixtures/languages/*.html tools/embed-bench/fixtures/bidi.html \
  --output tools/embed-bench/results/layout-languages-new.json
```

The font receives a private family name to avoid a collision with the installed
Droid font. Its glyphs and metrics are retained. Both hosts retain access to
system fonts, and no fonts are installed globally. `--inspect-fonts` records the
browser's paragraph fonts after the timing and memory readings. This checks
font selection, not text placement or line breaking equivalence.
