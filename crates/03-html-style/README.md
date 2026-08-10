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
