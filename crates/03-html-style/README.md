# html-style

`html-style` is the UI-framework-independent CSS stage. Its public boundary
accepts an `html-dom::Document` plus CSS text and returns a `StyledDocument`;
Lightning CSS AST types stay private to the crate.

Internally the data flow is one-way:

```text
CSS text
  -> declaration compatibility preprocessing
  -> Lightning CSS parser output (`ParsedStylesheetSet`)
  -> effective-rule preparation (`PreparedRuleSet`)
  -> selector index
  -> matching and cascade
  -> `html-style-model::ComputedStyles`
```

The selector index and resolver accept only prepared effective-rule handles,
not raw stylesheet/rule coordinates. Preparation is therefore the single
place to add import expansion, conditional-rule evaluation, and cascade-layer
ranking. Property syntax and renderer capability are separate public queries,
so valid-but-unimplemented CSS cannot be mistaken for invalid syntax.

Preparation borrows parser AST nodes instead of cloning them. Its rule handles
are four bytes, prepared metadata is capped by tests at 24 bytes per effective
rule, and matched-rule hot-path records are capped at 12 bytes. The preparation
timing is reported separately by `StyleTimings::prepare_rules` and propagated
through the pipeline profiler.

## Cascade validation

Run the reproducible resolver benchmark in release mode. Its arguments are the
element count, measured iterations, number of overriding rules, and important
declaration interval (`0` disables important declarations):

```sh
cargo run -p html-style --release --example cascade_benchmark -- 1500 25 48 8
```

The focused cascade integration tests use the repository's pinned Web Platform
Tests revision from `testdata/wpt-revision.txt`. Point `HTML_WPT_ROOT` at an
unmodified checkout of that revision:

```sh
HTML_WPT_ROOT=/absolute/path/to/wpt cargo test -p html-style --test wpt_css_cascade
```

For a broader end-to-end workload, run the CSS-only EPUB corpus benchmark. Its
arguments are measured iterations and the number of synthetic page copies:

```sh
cargo run -p html-style --release --example epub_css_benchmark -- 15 4
```
