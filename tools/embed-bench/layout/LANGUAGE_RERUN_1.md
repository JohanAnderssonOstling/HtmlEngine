# RTL and CJK rerun

One additional sample per fixture and host, 2026-10-26. The executable hashes, fixture hashes, private Fontconfig configuration, viewport and measurement procedure match the preceding language comparison. Every fixture uses a fresh host process and an initialized empty document baseline. System and filesystem caches were not cleared.

| Fixture | Skrift elapsed | Chromium elapsed | Skrift added PSS | Chromium added PSS |
| --- | ---: | ---: | ---: | ---: |
| Chinese | 18.78 ms | 11.30 ms | 9.01 MiB | 10.50 MiB |
| Japanese | 18.40 ms | 10.98 ms | 8.95 MiB | 10.46 MiB |
| Korean | 16.99 ms | 10.82 ms | 8.86 MiB | 10.53 MiB |
| Arabic RTL | 19.46 ms | 5.37 ms | 6.53 MiB | 4.14 MiB |
| Hebrew RTL | 16.40 ms | 4.91 ms | 6.32 MiB | 4.07 MiB |
| Mixed RTL | 25.51 ms | 17.72 ms | 6.51 MiB | 5.99 MiB |

| Fixture | Skrift engine CPU | Chromium engine CPU |
| --- | ---: | ---: |
| Chinese | 18.47 ms | 4.12 ms |
| Japanese | 18.27 ms | 3.72 ms |
| Korean | 16.86 ms | 3.39 ms |
| Arabic RTL | 19.21 ms | 1.93 ms |
| Hebrew RTL | 16.26 ms | 1.44 ms |
| Mixed RTL | 25.30 ms | 2.35 ms |

Chromium has lower elapsed time and lower measured engine CPU time in all six cases in this rerun. The earlier Japanese and Korean elapsed advantage for Skrift did not repeat. Chromium’s elapsed minus measured CPU gap fell from 17.20 to 7.25 ms for Japanese and from 14.60 to 7.43 ms for Korean. The measurements do not identify whether font access, work in other processes, cache state or scheduling caused the change.

Skrift retains a smaller added PSS in the CJK cases. These remain individual samples, and instrumentation boundaries differ between hosts. CPU time measures the selected engine intervals and calling threads rather than total process-session CPU consumption.

All twelve Chromium blank/document traces have zero paint or raster events. CJK paragraph font diagnostics confirm the private font is selected. Text placement and line breaking equivalence remain unverified, including the earlier mixed RTL placement issue.

- [Raw rerun measurements](../results/layout-languages-20261026-rerun-1.json)
- [Rerun traces](../results/layout-languages-20261026-rerun-1-traces/)
- [Original language measurements and methodology](LANGUAGE_RESULTS.md)
