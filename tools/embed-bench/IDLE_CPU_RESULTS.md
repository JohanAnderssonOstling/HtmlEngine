# Embedding CPU and RAM comparison

One sample per fixture and host, 2026-10-27. CPU covers all host threads and helper processes, including exited helpers, from an initialized idle window through completed viewport snapshot readiness. Wry uses WebKitGTK. Window startup is excluded. Snapshot readback is included; PNG encoding is excluded.

RAM is settled added PSS over the initialized empty window, including helper processes and proportionally shared resident pages. It is not total or peak memory and excludes GPU VRAM and the external compositor.

| Fixture | Skrift CPU (ms) | Wry CPU (ms) | Skrift added RAM (MiB) | Wry added RAM (MiB) |
| --- | ---: | ---: | ---: | ---: |
| Article | 46.05 | 27.19 | 10.39 | 6.46 |
| Images | 39.83 | 27.03 | 13.88 | 6.96 |
| Tables | 42.36 | 27.17 | 12.45 | 7.35 |
| Mixed RTL | 34.35 | 25.54 | 10.49 | 4.67 |
| Chinese | 29.50 | 19.54 | 13.09 | 8.70 |
| Japanese | 28.48 | 20.12 | 13.40 | 7.05 |
| Korean | 30.18 | 19.20 | 12.04 | 5.82 |
| Arabic | 23.14 | 14.13 | 10.57 | 4.48 |
| Hebrew | 22.81 | 13.95 | 10.48 | 4.45 |

Skrift used 1.35–1.69 times Wry’s total host CPU time in this run. Both hosts produced correctly sized rendered snapshots; pixel equivalence remains unverified. These are single observations without a measurement of variability.

- [Raw measurements](results/window-idle-render-20261027.json)
- [Measurement scope and validation](IDLE_RENDER_RESULTS.md)
