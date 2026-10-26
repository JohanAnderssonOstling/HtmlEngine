# RTL width correction

The language comparison exposed a text geometry bug in the GPUI/Cosmic path. It did not come from paragraph optimization, hyphenation controls, font size or HTML whitespace.

The adapter derived logical character advances from glyph drawing positions. Its terminal boundary used the full line width, which represents the opposite edge for an RTL run. In the first Arabic paragraph the final period received a 1781.73 pixel advance instead of its 5.09 pixel font advance. Mixed direction boundaries also introduced artificial width jumps.

GPUI now preserves the native shaper’s cluster source ranges, advances and direction. The HTML adapter converts that data into HtmlEngine’s existing `ShapedTextRun` contract and retains character advances separately from caret positions. A visual caret jump at a bidi boundary does not become extra character width. Native cluster information also retains ligatures and trailing spaces. Providers without cluster metadata retain the existing fallback.

HtmlEngine continues to own HTML and CSS processing, bidi resolution, line breaking, justification and hyphenation. The UI adapter supplies font measurements and paint resources. No engine layout policy was moved into GPUI.

| Fixture | Earlier native lines | Corrected native lines | Chromium lines |
| --- | ---: | ---: | ---: |
| Arabic | 16 | 12 | 12 |
| Hebrew | 16 | 12 | 12 |
| Mixed directions, including heading | 7 | 6 | 6 |

Every paragraph in the Arabic and Hebrew fixtures now has the same source text line breaks as Chromium. All period advances in those fixtures are 5.0859375 pixels. These are geometry checks without painting; a new pixel comparison has not been performed.

Chinese, Japanese and Korean line totals remain 9, 12 and 12 respectively. Chinese source line breaks match Chromium. Japanese and Korean still have some different source break positions, which require separate font and line breaking investigation. Matching totals alone does not establish equivalent output.

The adapter library test suite passes all 57 tests. New regression coverage checks RTL terminal carets and punctuation advances, mixed direction advance preservation, ligature and whitespace geometry, and cluster metadata when splitting a layout. An older fragment test was updated to inspect justified spacing through the borrowed document API rather than attempting to mutate that borrowed document.

This fix was validated on the Cosmic text backend used by the Linux benchmark. Other native font backends do not yet supply this new cluster metadata and were not validated by these checks. Earlier benchmark executables and result reports are retained; their RTL performance samples describe the earlier incorrect geometry.

- [Final executable and validation record](../results/layout-rtl-fix-validation-20261026.json)
- [Original native geometry](../results/layout-rtl-native-lines-20261026.json)
- [Corrected native geometry](../results/layout-rtl-fixed-native-lines-20261026.json)
- [Source line break comparison](../results/layout-rtl-fixed-line-breaks-20261026.json)
- [Corrected mixed direction geometry](../results/layout-rtl-fixed-mixed-audit-20261026.json)

For diagnostic line geometry, build the separate `embed-bench-layout-inspect` executable and run it with `SKRIFT_BENCH_INSPECT=1`. The original saved benchmark executable remains separate.

The final corrected executable is saved separately as `../gpui/target/release/embed-bench-layout-rtl-fixed`.
