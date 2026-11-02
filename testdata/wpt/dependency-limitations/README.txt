WPT dependency limitations and local fixes
Recorded 2026-11-02
WPT revision: 7ea3d665ff9ae1df1f742433d51bd1e9c6499a3b

Lightning CSS remains unchanged and the only CSS parser. No parallel source
parser or replacement grammar is used. Native AST inspection and reuse of
existing native typed value parsers remain allowed. This is an informational
registry; the runner does not consume it as an ignore or exclusion list.

Current result

Percentage letter/word spacing now uses the existing native LengthPercentage
parser and TextSpacing conversion. This fixes 16 more assertions without new
mismatch keys. Current selected syntax result: 16,467 / 18,469 passing (89.16%),
up from 16,451 / 18,469 (89.07%). Selection is unchanged.

Read spacing-cache-report.txt for the latest performance repair. Mixed native
spacing expressions are retained during preparation and reused during styling.
spacing-cache-performance.json and its compressed runs file retain all 630 fresh
process runs (300 final, 300 initial, 30 normalization repeats), including
allocation and peak-memory observations. Both builds apply the same features
and share current freshly rebuilt support crates.
304 style tests pass; the same pre-existing font-metric test fails before and
after. The 32 declaration and five reader font-scaling checks pass.

spacing-fix-report.txt retains the historical 16 fixes and their added-feature
costs; its older support-source snapshot is not the latest controlled baseline.

All earlier local-fix and selector reports retain their original stage scope.
Specification-version questions remain in selector-specification-review.json
and selector-followup-review.json; no expectations were changed for them.

Registry status

  178 blocked under current constraints
1,824 still needing investigation
  ---
2,002 current mismatches

  234 fixed_in_syntax_audit entries retained for provenance
2,236 original mismatch records total, across 189 original files

css-syntax.jsonl retains original keys, arguments and native observations.
Its status and resolution describe the latest outcome. Original actual/native
fields and investigation details remain historical. The 42 earlier fixes absent
from the original saved mismatch set remain excluded.

Confirmed blocks comprise 169 distinct-source native Property AST collisions,
two @scope recovery losses, one :has recovery loss and six slotted selector
chains rejected by a fixed native whitelist. They require native behavior or
provenance beyond the inspected AST-only path, not permanent impossibility.
blocked-css-syntax.txt retains their exact 178 keys. Unparsed or Custom alone
does not establish impossibility for the 1,824 unresolved assertions.

Rendering remains separate: rendering.json records two palette descriptor
candidates, five author font-variation integration cases and eleven flex-wrap:
balance cases (ten saved failures and one incidental image match). No rendering
case is certified here as an unavoidable dependency block. No image suite was
rerun for the local syntax fixes. The local GPUI fork already has font width,
slant and palette extensions.

Scope

- Counts are syntax assertions, not distinct bugs or whole-file WPT passes.
  Extracted computed-style helpers still check syntax, not computed output.
- Do not pool syntax counts with rendering-file counts or change overall WPT
  coverage denominators using this registry. The 13,434 currently unrepresented
  assertions and other static rendering failures remain unclassified here.
- No expectations, ignores, capability exclusions, or image comparison rules
  were changed. No test code, Lightning CSS or GPUI changes were added.
- Historical source hashes retain snapshot meaning; local-fix-report.json has
  first-stage production source hashes. spacing-cache-report.json records the
  latest performance delta; spacing-fix-report.json records the syntax delta; selector-cache-report.json has the preceding selector
  delta. Recheck findings after relevant changes.

Main files

  summary.json                         Counts, definitions, hashes and validation.
  README.txt                           This guide.
  css-syntax.jsonl                      All 2,236 original assertion records.
  blocked-css-syntax.txt                Exact 178 confirmed-block keys.
  spacing-cache-report.txt              Latest expression-cache performance and limitations.
  spacing-cache-report.json             Latest source/build/audit provenance.
  spacing-cache-performance.json        All 300-run timing/allocation comparisons.
  spacing-cache-performance-runs.jsonl.gz All 630 raw process runs.
  spacing-cache-after-mismatches.jsonl   Fresh unchanged 2,002 mismatch observations.
  spacing-fix-report.txt                Historical 16 fixes and performance findings.
  spacing-fix-report.json               Historical syntax delta and source hashes.
  spacing-fix-results.jsonl             Exact 16 new fixed assertions.
  spacing-fix-after-mismatches.jsonl     Exact 2,002 current mismatch observations.
  spacing-fix-performance.json          Historical spacing timing/allocation comparisons.
  spacing-fix-performance-runs.jsonl.gz  All 600 spacing-stage raw process runs.
  selector-cache-report.txt             Preceding cache changes and performance findings.
  selector-cache-report.json            Preceding cache-stage source hashes and scope.
  selector-cache-performance.json       Preceding cache-stage comparisons.
  selector-cache-performance-runs.jsonl.gz All 330 cache-stage raw process runs.
  selector-followup-review.json         Further specification-version questions.
  selector-fix-report.txt               Preceding 45 fixes and performance findings.
  selector-fix-report.json              Selector-stage audit, scope and hashes.
  selector-fix-results.jsonl            Exact 45 additional fixes.
  selector-fix-after-mismatches.jsonl   Preceding selector-stage 2,018 mismatches.
  selector-fix-performance.json         Complete selector-stage comparisons.
  selector-fix-performance-runs.jsonl.gz All 240 selector-stage raw process runs.
  selector-specification-review.json    Version-review notes and exact keys.
  local-fix-report.txt                  First-stage fixes and performance findings.
  local-fix-report.json                 Audit scope, counts and production hashes.
  local-fix-results.jsonl               Exact 173 fixed assertions and outcomes.
  local-fix-after-mismatches.jsonl      First-stage 2,063 mismatch observations.
  local-fix-performance.json            Methods and complete comparison summaries.
  local-fix-performance-runs.jsonl.gz   Raw final and intermediate measurements.
  investigation-report.txt             Historical addressability investigation.
  investigation-*.json/jsonl            Retained native/source investigation evidence.
  native-parser-evidence.jsonl          Historical native property/scope observations.
  selector-parser-evidence.jsonl        Historical selector observations.
  rendering.json                       Separate saved image-comparison cases.

Dependencies inspected: Lightning CSS 1.0.0-alpha.72, Parcel selectors 0.28.3,
Taffy 0.13.0 and the local GPUI fork declared 0.2.2.
