# Embedding benchmark

Measure one document load per fixture in an existing empty window. The window,
renderer and empty document are ready before measurement starts. Both hosts use
an 800 × 600 logical viewport and the same HTML fixtures and installed fonts.
Both use X11 at scale 1 by default, producing an 800 × 600 pixel snapshot.
`--scale-factor 2` selects matching high density snapshots for both hosts.

- Added time: receiving the load command through a completed viewport snapshot,
  including file reading, parsing, styling, layout, image decoding, painting and
  GPU readback. Window startup and PNG encoding are excluded.
- Added memory: process session PSS after loading minus PSS of the empty window.
  Both readings follow a 500 ms settling interval. This includes helper processes
  and shared memory proportionally, but does not cover all GPU VRAM or the desktop
  compositor. It measures retained memory, not peak allocation.
- One sample per host and fixture, with no warmup loads or extra timing/memory runs.
  Raw baseline and loaded memory counters are retained in the JSON for auditing.

With `--cpu-cgroup-parent`, the runner also measures CPU across every host
thread and helper process in a private Linux cgroup. CPU consumed by helpers
that exit is retained in the counter. The interval starts immediately before
sending the load command from the settled empty window and ends when snapshot
readiness reaches the runner. The same controller boundary supplies elapsed
time for both hosts, including command and notification delivery. CPU and time
stop before the memory settling interval. No browser tracing is enabled.

This mode requires a writable delegated cgroup v2 parent. For example:

```sh
python3 tools/embed-bench/run.py \
  --host wry=tools/embed-bench/wry/target/release/embed-bench-wry \
  --host skrift=tools/embed-bench/gpui/target/release/embed-bench-gpui-fixed \
  --cpu-cgroup-parent /sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service \
  --output tools/embed-bench/results/window-new.json
```

Use the delegated parent for your user ID. Each host joins its new group before
creating helpers. The runner removes its private group after the host exits.
PNG saving is disabled in this mode because encoding would otherwise enter the
CPU interval. Existing report files are never overwritten.
CPU-enabled summaries show total CPU time and added PSS. The current comparison
in that format is in [IDLE_CPU_RESULTS.md](IDLE_CPU_RESULTS.md).

Wry uses the platform webview. On this Linux machine that is WebKitGTK, so these
results do not compare against Chromium. Skrift uses the engine pipeline and
the application's GPUI shaping and painting backend, in a real native window. Decoded image dimensions feed back into layout
and text is painted using the shaped native runs.
It keeps the engine, decoded images and paint commands alive after loading.

Build the Wry host:

```sh
cargo build --release --manifest-path tools/embed-bench/wry/Cargo.toml
```

The GPUI host currently uses sibling `../GPUI-Fork` and `../Html-Prototype`
checkouts. Its native X11 GPU snapshot API is enabled with the `snapshot` Cargo
feature, without GPUI's test support or mock windows. The required GPUI changes
are also saved in `gpui/native-snapshot.patch`; apply that patch to a fresh fork
checkout before building. The current local checkout already has it applied.

```sh
cargo build --release --manifest-path tools/embed-bench/gpui/Cargo.toml
python3 tools/embed-bench/run.py \
  --host wry=tools/embed-bench/wry/target/release/embed-bench-wry \
  --host skrift=tools/embed-bench/gpui/target/release/embed-bench-gpui \
  --output tools/embed-bench/results/comparison.json
```

For visual inspection, `--screenshots DIRECTORY` saves each document's snapshot
in the same run. PNG encoding happens after the timed endpoint but can affect
retained memory; omit it when comparing memory. Check that all expected content
is present before interpreting cross-engine timings. Each engine's own viewport
snapshot implementation contributes to the result. A single sample gives an
initial observation without measuring run-to-run variability.

The initial measurements and visual findings are in [RESULTS.md](RESULTS.md).
The corrected host's comparison from idle window to rendered pixels, including
whole host CPU accounting, is in [IDLE_RENDER_RESULTS.md](IDLE_RENDER_RESULTS.md).

A separate comparison through layout, without painting or GPU readback, is
available in [layout/README.md](layout/README.md). It writes new report files
and retains the native window results above.
