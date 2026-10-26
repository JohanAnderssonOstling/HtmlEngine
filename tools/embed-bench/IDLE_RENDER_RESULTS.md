# Idle window to rendered output

One sample per fixture and host on 2026-10-27. Both hosts use a native X11 window, an 800 × 600 viewport at scale 1, the same input HTML and a shared Fontconfig file. Skrift uses the rebuilt GPUI host with the RTL cluster width correction. Wry uses WebKitGTK, not Chromium.

The empty document is rendered first and allowed to settle for 500 ms. The timed interval starts immediately before the controller sends the document load command and ends when a completed viewport snapshot notification reaches the controller. It includes file reading, parsing, font and image work, shaping, layout, painting, snapshot readback and command/notification delivery. Window startup is excluded. No PNG is encoded. This is completed viewport pixel output, not a measurement of physical screen presentation.

Total CPU is the increase in a dedicated cgroup v2 usage counter over that same interval. The host joins its private group before it can create helpers. Every host thread and helper is charged, including helpers that exit. The runner remains outside the measured group. No selected-stage profiler or browser tracing is used. GPU execution time and external display server/compositor CPU are not counted as host CPU. Parallel helper work can make CPU time exceed elapsed time.

| Fixture | Skrift total CPU | Wry total CPU | Skrift / Wry |
| --- | ---: | ---: | ---: |
| Article | 46.05 ms | 27.19 ms | 1.69× |
| Mixed RTL | 34.35 ms | 25.54 ms | 1.35× |
| Images | 39.83 ms | 27.03 ms | 1.47× |
| Tables | 42.36 ms | 27.17 ms | 1.56× |
| Chinese | 29.50 ms | 19.54 ms | 1.51× |
| Japanese | 28.48 ms | 20.12 ms | 1.42× |
| Korean | 30.18 ms | 19.20 ms | 1.57× |
| Arabic | 23.14 ms | 14.13 ms | 1.64× |
| Hebrew | 22.81 ms | 13.95 ms | 1.63× |

Skrift used 1.35–1.69 times Wry’s total host CPU time in these observations. The underlying web engine and measurement scope differ from the earlier headless Chromium stage comparison, so the two sets of ratios do not isolate a single cause.

| Fixture | Skrift elapsed | Wry elapsed | Skrift added PSS | Wry added PSS |
| --- | ---: | ---: | ---: | ---: |
| Article | 51.42 ms | 34.75 ms | 10.39 MiB | 6.46 MiB |
| Mixed RTL | 53.69 ms | 27.63 ms | 10.49 MiB | 4.67 MiB |
| Images | 49.90 ms | 28.25 ms | 13.88 MiB | 6.96 MiB |
| Tables | 47.90 ms | 23.20 ms | 12.45 MiB | 7.35 MiB |
| Chinese | 36.30 ms | 22.80 ms | 13.09 MiB | 8.70 MiB |
| Japanese | 31.62 ms | 23.08 ms | 13.40 MiB | 7.05 MiB |
| Korean | 41.29 ms | 22.36 ms | 12.04 MiB | 5.82 MiB |
| Arabic | 27.19 ms | 9.37 ms | 10.57 MiB | 4.48 MiB |
| Hebrew | 27.08 ms | 9.21 ms | 10.48 MiB | 4.45 MiB |

PSS is sampled 500 ms after snapshot readiness and compared with settled empty-window PSS. It includes processes in the private host session and shared pages proportionally. It is retained memory rather than peak memory, and excludes GPU VRAM and the external compositor. The CPU and elapsed counters stop before this memory settling period.

Both hosts produced correctly sized snapshots for every fixture. The earlier headless geometry checks established matching Arabic/Hebrew source line breaks against Chromium after the correction. This run does not establish pixel equivalence with WebKitGTK: actual font fallback and text placement can still differ, including Japanese and Korean breaks. Different snapshot implementations contribute to the measured embedding cost. These remain individual samples without a measurement of variability.

Older reports and measured binaries are unchanged. The new native host also incorporates the current local engine and adapter sources; this is a fresh observation, not a controlled attribution of performance changes to the RTL fix alone.

- [Raw measurements, CPU counters, process IDs and hashes](results/window-idle-render-20261027.json)
- [Preserved earlier result hashes](results/window-idle-render-20261027-preserved-results.json)
- [Run instructions and scope](README.md)
- [Earlier window results](RESULTS.md)
- [Corrected headless stage results](layout/FIXED_CPU_RESULTS.md)
