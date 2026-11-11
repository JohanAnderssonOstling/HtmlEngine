# Layout stage architecture review

10 November 2026. Focus: improving simplicity without compromising performance.

Scope: the layout crate (`crates/04-html-layout`) and the pipeline's layout invocation (`crates/html-pipeline/src/stages/lifecycle.rs`), read from source only. Nothing was compiled or run.

The stage structure itself (numbered stages, formatting contexts returning `ContentLayoutResult`, retained placement transforms flattened once in `finalize`) is sound. The avoidable complexity is in the plumbing around it.

The engine, box pipeline, placement, finalization and paint replay were read closely. For inline tokens/emission, table internals and Knuth–Plass only the module roots and outlines were read, so there are no findings there.

Paths below are relative to `crates/04-html-layout/src/` unless stated otherwise. Line numbers are approximate.

## Highest value

### 1. Scratch ownership — simplified

`LayoutEngine` now borrows `LayoutScratch` and `PlacementState` directly.
`LayoutOutputs` uses one output lifetime, and pass setup/teardown no longer
moves each scratch field into and out of the engine. Isolated measurement
workers continue to own their own scratch state.

The bounded hyphenation dictionary cache now lives in scratch and survives
successive passes. Its key includes word, language and quality. Inline-plan
transfers needed for recursive measurement and token replay remain. The
decoration-buffer handoff was subsequently removed in item 5.

Changes were reviewed from source and checked for Rust syntax and whitespace.
No compilation, tests or benchmarks were run.

### 2. Final layout and measurement probes — explicit

`execute_layout` now takes `LayoutPassKind::Final` or `LayoutPassKind::Probe`.
The pass kind controls inline-plan trimming, placement reset/output retention,
paragraph cache slots and probe fragmentation suppression. Paint recording
continues to use placement output retention, now derived from the explicit pass
kind rather than the geometry storage strategy.

The fragmentation depth counter only tracks nested `without_fragmentation`
scopes. Measurement workers retain lazy geometry reset as a storage optimization.
`BoxMeasurementMode` remains the per-request sizing policy.

Changes were reviewed from source and checked for Rust syntax and whitespace.
No compilation, tests or benchmarks were run.

### 3. Formatting-context contracts — simplified

Block flow and inline box layout now consume `&ResolvedBoxSizing` and a content
point, alongside their child range and indentation policy. Table layout already
used this contract. Flex/grid dispatch consumes resolved sizing and passes one
`FlexGridInput` to its solver coordinator. The argument-forwarding
`formatting_context.rs` module has been removed, and the two natural auto-height
flex/grid probes share one helper.

### 4. Layout/relayout entry points — consolidated

`ShapedDocument::layout(request, shaper, resources)` and
`LaidOutDocument::relayout(request, shaper, resources)` are now the two public
entry points for laying out already-shaped text. `LayoutRequest` carries
constraints and optional image metrics. The metrics/shaper wrapper variants
and shaper-less paths have been removed, and callers use the request API.
Ordinary relayout retains its allocation-reusing path; first-line styles still
use shaping refinement. Resource identity and generation checks remain.

Callers and existing fixtures were migrated from source only. No compilation,
tests or benchmarks were run.

## Medium

### 5. Block decoration ownership — one output vector

The fragment writer appends owners directly to
`state.fragment_output.block_decoration_owners`. Replay clears and reuses this
vector, with no scratch handoff or copy. Inline owners remain available during
replay for clipping, then the vector is truncated to the block prefix to preserve
the published ownership contract. The writer constructor no longer takes a
separate owner vector.

### 6. Shared pass context — implemented; performance validation pending

`PassContext` replaces the misleading `BoxStyleSnapshot` name. Its paint-only
constructor prepares style views and document flags in one traversal, skipping
box-model/size caches, intrinsic-width slots and paragraph-spacing preparation.
Normal layout still prepares those once and shares them with its probes.

Flex/grid measurement now uses one shared cache. The engine-local cache and its
per-probe reset have been removed. The shared lock is released before recursive
measurement. This removes duplicate lookups and storage but makes each cache hit
acquire the shared lock; its performance has **not** been benchmarked. The
original recommendation required before/after benchmarks, so this finding is
not fully closed.

### 7. Layout instrumentation — removed

The custom layout duration collector, work counters, profiling scopes and
Knuth–Plass diagnostic probes have been removed, including their API wrappers
and benchmark reporting. Benchmark-level elapsed times remain; use external
`perf` sampling for internal hotspots. Add targeted instrumentation only when
there is a concrete need. This removal was reviewed from source and syntax only;
no compilation, tests or benchmarks were run.

## Small — completed

- Removed the hardcoded `force_justify` option, branches and paragraph-cache field.
- Removed repaint's unused constraints argument and migrated its caller.
- Inlined final-pass setup into `compute_layout`; removed `with_engine` and migrated its existing fixture.
- Internal model imports now use `html_engine_model`; public compatibility re-exports remain.
- Restored numeric stage filenames and aligned module names with their stems (for example `03-finalize.rs` → `s03_finalize`). Box-stage modules use a `box_` prefix because they share the parent layout module with the document stages. Module declarations follow numeric order.
- Layout executor parameters consistently use `engine`.

## Validation and remaining work

The code changes for all findings have been implemented. Item 6 remains open
for the explicitly recommended before/after performance comparison. Source
review, Rust syntax parsing and whitespace checks were performed. No compilation,
tests or benchmarks were run, per the repository instructions. Type/borrow
checking and behavioral verification also remain outstanding.

Flex/grid uses a `FlexGridInput` at the solver boundary rather than passing
`ResolvedBoxSizing` through that boundary. Numeric filename prefixes have been
restored and the module names now reflect them, as originally recommended.

Stage numbers describe the reading order within each pipeline. Stage 00 is its
coordinator, not an independent pass before all other stages. Document layout
runs preparation, root layout, finalization and paint replay in that order. Child
layout recurses, block flow repeats its stages per child, cached inline plans can
skip preparation/composition work, and flex/table probes can revisit layout.
Paint-only replay goes directly to stage 04; execution is not globally linear.
