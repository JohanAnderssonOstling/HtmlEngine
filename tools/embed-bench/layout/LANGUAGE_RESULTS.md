# RTL and CJK layout comparison

One sample per fixture and host, 2026-10-26. These are additional results; the earlier window and layout reports remain unchanged.

The viewport is 800 × 600 at scale 1. Each host loads an empty document before the measured document. Memory is the settled process-session PSS increase over that empty document. No window or GPU device is created by Skrift. Chromium uses manual BeginFrame control and all twelve blank/document traces report zero paint, raster or drawing events.

| Fixture | Skrift elapsed | Chromium elapsed | Skrift added PSS | Chromium added PSS |
| --- | ---: | ---: | ---: | ---: |
| Chinese | 28.08 ms | 25.63 ms | 8.78 MiB | 10.16 MiB |
| Japanese | 16.64 ms | 22.15 ms | 8.23 MiB | 10.17 MiB |
| Korean | 17.35 ms | 18.52 ms | 8.69 MiB | 10.22 MiB |
| Arabic RTL | 16.35 ms | 5.70 ms | 6.14 MiB | 4.09 MiB |
| Hebrew RTL | 14.97 ms | 5.30 ms | 6.14 MiB | 3.98 MiB |
| Mixed RTL | 40.04 ms | 18.57 ms | 5.93 MiB | 5.55 MiB |

Elapsed time covers Skrift preparation, shaping and layout calls, and Chromium traced parse/style/layout intervals. The instrumentation boundaries differ. First document font use is included; file reading, protocol overhead and gaps between stages are excluded. These fixtures differ in length, so compare hosts within a row rather than comparing languages against one another.

| Fixture | Skrift engine CPU | Chromium engine CPU |
| --- | ---: | ---: |
| Chinese | 19.30 ms | 5.81 ms |
| Japanese | 16.50 ms | 4.96 ms |
| Korean | 17.25 ms | 3.92 ms |
| Arabic RTL | 16.25 ms | 2.30 ms |
| Hebrew RTL | 14.88 ms | 2.26 ms |
| Mixed RTL | 31.34 ms | 3.17 ms |

Skrift has a shorter elapsed interval in the Japanese and Korean samples, while Chromium has lower measured engine CPU time in every sample. Scheduling, font access and trace boundaries contribute to the difference between CPU and elapsed time. One sample cannot establish stable rankings.

## Font and text coverage

Chinese and Japanese contain unspaced text, punctuation and mixed Latin text and digits. Korean contains Hangul, spaces, punctuation and mixed Latin text and digits. Arabic and Hebrew use explicit right to left document direction, mixed Latin names, numbers and punctuation. The original mixed direction fixture is also retained.

The installed Droid CJK font on this machine lacks Hangul coverage. Both hosts instead receive a private copy of the application’s bundled `DroidSansFallback.ttf`, with the family renamed to `Skrift Benchmark CJK` to avoid that name collision. Glyph outlines, character mappings and horizontal metrics were checked against the original. System fonts remain available. DejaVu Sans supplies Latin punctuation and text where needed. No system font installation was changed.

Character map checks found no uncovered codepoints in the five new fixtures using the private CJK font and DejaVu Sans together. Browser font diagnostics, collected after timing and memory readings, confirm the private CJK font is used in every CJK paragraph and DejaVu Sans is used for Arabic and Hebrew. The native host receives the same Fontconfig file; its actual glyph font IDs were not independently inspected.

Both hosts completed layout. These are performance diagnostics, not proof of identical shaping, line breaks or text placement. The earlier window comparison found a mixed RTL placement mismatch, and this layout only run does not establish a fix. CJK has not received a pixel comparison either.

## Reproducibility

The saved Skrift executable is identical to the one used for the earlier verified layout report. Chromium is the cached headless shell, version HeadlessChrome/149.0.7827.55.

- [Raw measurements and paragraph font diagnostics](../results/layout-languages-20261026.json)
- [Raw Chromium traces](../results/layout-languages-20261026-traces/)
- [Font provenance](../results/language-fonts-20261026/manifest.json)
- [Font character coverage](../results/language-fonts-20261026/coverage.json)
- [Preparation and run commands](README.md)
- [Earlier layout report](RESULTS.md)
- [Earlier window report](../RESULTS.md)

Executable SHA256 hashes:

- skrift: `3e6eddfed9b626602b3861762ecc48b39ab94fb1038722a10d1bf713d1c4ad97`
- chromium: `670ba079b75107746ba41abad131180a31a7c7219aa1bd4061fb471f4535d541`
