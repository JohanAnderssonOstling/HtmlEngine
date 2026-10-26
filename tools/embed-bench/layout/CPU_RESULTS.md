# Engine CPU time comparison

The existing language rerun already measured CPU time alongside elapsed time. This report presents those same samples using engine CPU time as the primary comparison; no additional loads were performed.

| Fixture | Skrift engine CPU | Chromium engine CPU | Skrift / Chromium |
| --- | ---: | ---: | ---: |
| Chinese | 18.47 ms | 4.12 ms | 4.48× |
| Japanese | 18.27 ms | 3.72 ms | 4.91× |
| Korean | 16.86 ms | 3.39 ms | 4.97× |
| Arabic RTL | 19.21 ms | 1.93 ms | 9.96× |
| Hebrew RTL | 16.26 ms | 1.44 ms | 11.32× |
| Mixed RTL | 25.30 ms | 2.35 ms | 10.77× |

Skrift measures calling-thread CPU time with `CLOCK_THREAD_CPUTIME_ID` around engine preparation, shaping and layout calls. Chromium uses thread CPU timestamps for traced parse, style and layout stages, merging overlapping intervals per thread. Waiting and time descheduled are excluded. These counters do not measure total CPU across the process session, including all helper threads and processes. The stage boundaries also differ between engines.

Chromium used less measured engine CPU time in every fixture in this rerun. One sample per host and fixture is retained; these numbers are observations rather than a statistical comparison. Layout correctness caveats, including the earlier mixed RTL placement mismatch, still apply.

- [Raw measurements](../results/layout-languages-20261026-rerun-1.json)
- [Elapsed time, memory and rerun details](LANGUAGE_RERUN_1.md)
- [Benchmark methodology](README.md)
