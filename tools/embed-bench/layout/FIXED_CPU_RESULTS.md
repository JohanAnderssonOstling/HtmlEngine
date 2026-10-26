# CPU comparison after the RTL width correction

One sample per language fixture and host, 2026-10-27. The saved corrected native executable is used; Chromium, fixture files, viewport and shared font configuration match the previous language rerun. No painting or rasterization occurs. All earlier reports and executable files are retained.

| Fixture | Skrift engine CPU | Chromium engine CPU | Skrift / Chromium |
| --- | ---: | ---: | ---: |
| Chinese | 18.34 ms | 5.36 ms | 3.42× |
| Japanese | 19.28 ms | 4.66 ms | 4.14× |
| Korean | 22.80 ms | 4.31 ms | 5.29× |
| Arabic | 17.34 ms | 3.00 ms | 5.79× |
| Hebrew | 16.97 ms | 2.29 ms | 7.41× |
| Mixed RTL | 33.65 ms | 4.71 ms | 7.14× |

Chromium has lower measured engine CPU time in every fixture. Fixing the RTL geometry does not remove this observed CPU gap. Individual before and after samples do not establish the performance effect of the fix.

| Fixture | Skrift elapsed | Chromium elapsed | Skrift added PSS | Chromium added PSS |
| --- | ---: | ---: | ---: | ---: |
| Chinese | 19.95 ms | 16.77 ms | 7.15 MiB | 9.65 MiB |
| Japanese | 20.45 ms | 13.79 ms | 7.03 MiB | 10.00 MiB |
| Korean | 24.01 ms | 15.23 ms | 7.29 MiB | 7.22 MiB |
| Arabic | 17.70 ms | 8.19 ms | 6.76 MiB | 3.11 MiB |
| Hebrew | 17.11 ms | 6.87 ms | 6.46 MiB | 3.74 MiB |
| Mixed RTL | 34.42 ms | 28.37 ms | 6.15 MiB | 5.21 MiB |

Added PSS is the settled process-session memory increase over an initialized empty document. These are retained memory readings rather than peaks; Chromium tracing allocations can contribute. System and filesystem caches were not cleared.

The corrected native executable reports 12 lines for each Arabic and Hebrew fixture and 6 lines including the heading for the mixed direction fixture. The prior geometry checks on this exact executable verified identical Arabic and Hebrew paragraph line breaks against Chromium. CJK totals remain 9, 12 and 12, but Japanese and Korean still have some different source break positions. No new pixel comparison is claimed.

CPU boundaries remain approximate: Skrift calling-thread CPU inside full preparation/shaping/layout calls is compared with Chromium thread CPU inside selected parse/style/layout trace intervals. Work in browser helper processes and other untraced stages is excluded. These numbers are not a comparison of total process tree CPU.

- [Raw measurements and font diagnostics](../results/layout-languages-rtl-fixed-20261027.json)
- [Raw browser traces](../results/layout-languages-rtl-fixed-20261027-traces/)
- [RTL correction and correctness checks](RTL_FIX.md)
- [Previous CPU results](CPU_RESULTS.md)
- [Measurement differences](COMPARABILITY_AUDIT.md)

Executable SHA256:

- skrift: `a70f8b727fcad2abf2ff02d9ab98a2a489679dba72e72bc5c0c94552b98b5bbf`
- chromium: `670ba079b75107746ba41abad131180a31a7c7219aa1bd4061fb471f4535d541`
