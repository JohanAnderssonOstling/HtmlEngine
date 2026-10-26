# Initial comparison through layout

One sample per host and fixture on 2026-10-26, using an initialized empty
document as the memory baseline. Viewport: 800 × 600 at scale 1. Neither host
requires a display server. Skrift creates no window or GPU device. Chromium
headless shell 149.0.7827.55 uses manual frame control; every captured trace
contains zero paint, raster or draw events. SVG prepaint bookkeeping still occurs.

Times below are elapsed engine stages through layout, excluding painting, GPU
readback, control transport and gaps between stages. They differ from the older
native window benchmark's load-to-snapshot endpoint. CPU timings and end-to-end
load-to-layout diagnostics are recorded separately in the JSON.

| Fixture | Skrift elapsed | Chromium elapsed | Skrift added PSS | Chromium added PSS |
| --- | ---: | ---: | ---: | ---: |
| Article | 36.59 ms | 11.94 ms | 5.97 MiB | 6.66 MiB |
| Images | 27.02 ms | 7.94 ms | 5.63 MiB | 6.70 MiB |
| Tables | 36.06 ms | 18.75 ms | 7.58 MiB | 6.30 MiB |

Skrift took longer in these single observations. Its retained PSS increase was
lower for the article and images and higher for the table. This run uses
Chromium, while the older Wry test used WebKitGTK. Subtracting the two sets of
timings would not isolate the cost of painting.

The CPU observations, which exclude time spent waiting, were:

| Fixture | Skrift engine CPU | Chromium engine CPU |
| --- | ---: | ---: |
| Article | 36.08 ms | 3.84 ms |
| Images | 26.67 ms | 2.59 ms |
| Tables | 35.47 ms | 6.98 ms |

Most measured Skrift CPU time is in the combined shaping/layout phase, which
includes first use of document fonts. This does not isolate the cause within
that phase. Browser helper work outside traced stages is not included in its
CPU number; the instrumentation boundaries are approximate matches.

Both hosts laid out all 81 table rows and both SVG images with matching
intrinsic dimensions and rectangles (within half a CSS pixel). This checks
geometry without generating pixels. The mixed RTL fixture completed too, but
the prior text placement mismatch remains a correctness concern. Its diagnostic
measurements are in the raw report; these runs do not establish visual equality.

Memory is process-session PSS after a 500 ms settling interval, minus PSS of
the settled empty document. Browser trace collection is complete before reading
memory, but retained instrumentation allocations can contribute. This measures
retained memory rather than allocation peaks.

- [Raw results](../results/layout-verified-20261026.json)
- [Browser traces](../results/layout-verified-20261026-traces)
- [Verified unchanged older artifact hashes](../results/layout-verified-20261026-preserved-window.json)
- [Older native window results](../RESULTS.md)
- [Build, run and timing boundaries](README.md)
