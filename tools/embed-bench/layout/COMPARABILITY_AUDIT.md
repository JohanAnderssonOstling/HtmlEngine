# Comparison audit

This audit examines differences in the existing language comparison. It performs untimed browser layout queries and does not replace any performance samples or earlier reports.

## Typography settings

The native benchmark explicitly selects `TextCompositionPolicy::WebCompatible`. Its language fixtures do not request justification or automatic hyphenation and contain no soft hyphens. Browser runtime checks show `hyphens: manual`, `text-align: start` and `text-wrap-style: auto`. Skrift’s book composition policy and optional hyphenation quality safeguards are not enabled for these fixtures. Complex script shaping and bidirectional processing still apply.

## Output differences

| Fixture | Skrift reported layout lines | Chromium text lines |
| --- | ---: | ---: |
| Chinese | 9 | 9 |
| Japanese | 12 | 12 |
| Korean | 12 | 12 |
| Arabic | 16 | 12 |
| Hebrew | 16 | 12 |

Browser line counts come from the vertical positions of character ranges within each paragraph; native counts are the saved engine layout line count. These counters require reconciliation for RTL: they do not establish whether the difference is visible wrapping, extra native line records or another layout issue. The CJK totals agree but do not establish identical break positions or shaped glyphs. The mixed direction page additionally contains a heading, so its native total is not directly compared to the paragraph only browser count. The earlier window RTL placement mismatch remains relevant.

## Measurement differences

- Native engine CPU covers full preparation and shape/layout calls on the calling thread. Browser engine CPU covers selected traced parse/style/layout intervals. Browser work in helper threads, services or other stages is excluded. These are not total process session CPU counters.
- Native shaping passes through the GPUI adapter and Cosmic text backend. Its font resolution, shaping, caches and text conversion costs differ from the browser implementation. Those are relevant embedding costs, but the result cannot be attributed solely to the HTML layout algorithm.
- Both hosts receive the same Fontconfig file and requested font families. Browser actual font use is checked; native actual glyph font selection has not been independently verified.
- Empty document initialization does not guarantee equally warm font or shaping caches. Font database loading occurs during native text system construction, before the baseline. First document font use is included; filesystem and system font caches are not cleared.
- Chromium tracing and DevTools instrumentation have no equivalent native instrumentation overhead. Native CPU counters themselves also have overhead. Retained tracing allocations can contribute to browser added PSS.
- Each report contains one sample per fixture and host. Scheduling and cache effects can move elapsed timings substantially; the Japanese and Korean elapsed advantage did not repeat.

Comparing total process session CPU over equivalent document load boundaries would improve the embedding cost measurement. It would still include different browser control overhead and would need that overhead documented. Text output should be reconciled before calling the RTL performance comparison equivalent.

- [Raw text and computed style audit](../results/layout-language-text-audit-20261026.json)
- [Existing CPU results](CPU_RESULTS.md)
- [Existing rerun](LANGUAGE_RERUN_1.md)
