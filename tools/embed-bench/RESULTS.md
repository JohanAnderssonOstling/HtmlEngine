# Initial embedding results

One sample per host and fixture on 2026-10-26. Both hosts use X11 with an
800 × 600 logical and physical viewport. Each document loads into an already
initialized empty window. Time ends after viewport snapshot readback; memory
is settled document PSS minus settled empty-window PSS. PNG encoding is excluded.

| Fixture | Skrift GPUI time | Wry WebKitGTK time | Skrift added PSS | Wry added PSS |
| --- | ---: | ---: | ---: | ---: |
| Article | 62.2 ms | 24.1 ms | 9.15 MiB | 5.81 MiB |
| Images | 42.3 ms | 16.6 ms | 12.11 MiB | 7.60 MiB |
| Tables | 43.8 ms | 24.4 ms | 10.28 MiB | 7.14 MiB |

The current Skrift embedding prototype used more added time and retained PSS
in these single observations. These are initial observations without a
measurement of variability. WebKitGTK is the Wry backend on this machine;
this is not a Chromium comparison.

Visual inspection found the expected article, images and table content.
Font rasterization and some collapsed table header borders differ. The mixed
RTL fixture still has incorrect text placement in Skrift and is excluded from
the table above. Its raw measurement remains in the JSON with `comparable: false`.

Raw counters, process IDs, binary and fixture hashes, viewport checks and
the visual review are in [comparison.json](results/comparison.json). The
validation screenshots are in [visual-images](results/visual-images). Image
validation confirms that Skrift paints both decoded SVG resources. GPU VRAM
and desktop compositor memory are not fully covered by PSS.

Build and run instructions are in [README.md](README.md).
