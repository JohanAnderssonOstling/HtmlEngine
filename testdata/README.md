# Web Platform Test Inputs

This repository owns the parser, CSS, and noninteractive-layout manifests and
failure ledgers under `wpt/`. Small fixtures used through `include_str!` are
vendored there so all test targets compile offline.

The direct layout runner excludes whole quirks and limited-quirks documents
through `wpt/skipped-quirks-layout-files.txt`; that list is mode-checked and
audited for stale or newly encountered exclusions.

Documents containing replaced elements are also outside the direct layout
runner's support boundary. They are detected from the parsed DOM, skipped as
whole files because their intrinsic sizes affect ancestor geometry, and held
to a pinned exclusion count.

Large unchanged upstream inputs are not duplicated. Set `HTML_WPT_ROOT` to the
absolute path of a checkout at the revision recorded in `wpt-revision.txt`
before running the WPT integration suites.
