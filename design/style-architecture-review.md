# Style stage architecture review

Date: 2026-11-10
Scope: `crates/03-html-style`, focusing on where simplicity can improve without costing performance.

This review was done by reading the code only. Nothing was compiled, run or benchmarked, so the performance notes are reasoned from the code, not measured.

Paths below are relative to `crates/03-html-style/src/style/`.

## Summary

The stage split (source → syntax → rules → matching → cascade) and the fragment / link / resolve layering are sound. Most of the complexity comes from three root causes, plus some structural debt:

1. Validity is decided during computed-value conversion, so the cascade is transactional.
2. The lightningcss AST is the runtime representation, with side tables bolted on.
3. Inline styles and presentational hints take a second, unprepared path.

## Root causes

### 1. Validity is decided during computed-value conversion

- Property appliers can return `ApplyResult::Invalid` (e.g. `position: fixed`, negative sizes in `05-cascade/resolver/properties/box_model.rs`).
- Because a winner can fail late, `apply_cascade` in `05-cascade/resolver/runner.rs` loops: select winners, apply, and on failure remove the event and redo selection and application from scratch.
- Every successful declaration also pays for this: `snapshot_groups`, compute into a scratch style, `swap_groups`, then `copy_computed` per winning target.
- Supporting machinery exists only for this:
  - the `property_is_always_computable` whitelist (`03-rules/prepared.rs:176`);
  - the tri-state eligibility bits in `PreparedTargetRange` (`compiled.rs`);
  - the same bits repacked as `declaration_flags` in `05-cascade/resolver/specified.rs`;
  - shadow masks gated on the whitelist.

**Proposal.** Make "a prepared declaration cannot fail" an invariant. Each `Invalid` site is either context-free (reject at preparation) or a range issue (clamp, which is also what CSS specifies for out-of-range `calc()`). Only `var()`-substituted values can then fail, and those already mean `unset`, with no retry.

**Effect.** The retry loop, the scratch style, the whitelist and both flag encodings go away, and winning longhands are applied directly. This is faster, but it is the riskiest change behaviourally.

**Care needed.** Applying only the winning longhands of a shorthand must stay exact: a `revert` rollback can let a lower-priority declaration win one longhand, so the shorthand must not overwrite it. Logical/physical aliases that share storage need the same attention.

### 2. The lightningcss AST is the runtime representation

- About 44 renderer-owned properties (`RendererPropertyId` in `02-syntax/property_id.rs`: `float`, `clear`, `break-*`, `content`, `font-feature-settings`, …) stay as token lists.
- `05-cascade/resolver/properties/unparsed.rs` re-parses them each time a declaration is applied to an element. `font-feature-settings` allocates and sorts each time.
- The one case already optimised, letter/word spacing, uses a map keyed by token-list address (`03-rules/prepared.rs:253`, `PreparedTextSpacing`) that is threaded through about 30 references.
- The same address-as-identity pattern recurs in:
  - the substitution cache (`runner.rs`);
  - the `:has()` result cache (`04-matching/dom.rs`);
  - `GeneralNames` (`04-matching/names.rs`);
  - `NthIndexKey` (`04-matching/dom.rs`).

**Proposal.** Parse these once at preparation into typed values in a per-fragment pool, parallel to `declaration_expansions`. The token path remains only after substitution.

**Effect.** Removes `PreparedTextSpacing`, removes per-element token parsing, and makes validity for these properties known at preparation, which feeds item 1 (the `Property::Custom(_) => None` "eligibility unknown" case in `02-syntax/capabilities.rs` disappears).

### 3. Inline styles and hints take a second, unprepared path

- Rule declarations carry precomputed targets, longhands and eligibility.
- Inline declarations are cached only as a raw `StyleAttribute` (`01-source/inline.rs`). They recompute targets inside `select_specified_values` (which sits inside the retry loop), and check eligibility and expand shorthands per element.
- `DeclarationSource` in `specified.rs` therefore has three variants (`Rule`, `Inline`, `Hint`) with `Option`-returning accessors (`prepared_targets`, `prepared_longhands`, `prepared_renderer_eligibility`, `rule_property`).

**Proposal.** Factor the per-block part of `PreparedStylesheetFragment` (targets, ranges, expansions, longhands, shadow masks) into a prepared declaration block, and store one per entry in `ParsedInlineStyleCache`.

**Effect.** One event shape, and a faster path for inline-heavy documents.

## Structural debt

### Interior mutability in matching

`SiblingIndex` (`04-matching/dom.rs:24`) has six `RefCell`/`OnceCell` fields and owns the traversal cursor, driven through `&self` (`next_element`). `StyleTraversal` adds an `OnceCell<RefCell<Facts>>`. The name no longer describes what the type does, and comments such as "do not retain a RefCell borrow while nested matching runs" show the cost.

**Proposal.** Split it into a `StyleTraversal` owned by the document loop and match caches passed as `&mut` through `MatchContext`. Performance is neutral to slightly positive.

### Resolver session borrows its own scratch

`StyleResolverContext` (`05-cascade/resolver/session.rs`) takes ten `&mut` scratch buffers that are declared a few lines above its construction (`05-cascade/resolver/mod.rs:90`). Methods then `mem::take` them and restore them on every exit path, and `last_pseudo_match` is a validity flag for the contents of two of those buffers.

**Proposal.** Let the session own its scratch, grouped into a matcher half (candidates, pseudo candidates, local match cache, specificities) and a cascade half (cascade scratch, declaration style, sharing and custom-property caches). Disjoint field borrows then replace take/restore.

### Duplicated candidate loop

The element and pseudo-element matching loops are near-identical (`mod.rs:345` and `mod.rs:517`). Merge them into one function taking `Option<PseudoTarget>`, and move the pseudo-element orchestration (`mod.rs:207`) out of the document loop.

### Linking copies fragment tables

`prepare` (`03-rules/prepared.rs:403`) copies targets, ranges and expansions from each fragment into global arrays and clones one `Arc` per rule for both the style rule and the longhand pool.

**Proposal.** Store `(fragment, local rule index)` per effective rule and read fragment tables directly.

**Effect.** Removes five vectors, `append_rule_declarations` and the text-spacing merge. Relinking on an environment change touches metadata only. The cost is one extra indirection per rule read in the cascade.

### Stylesheet identity is text, three times

In the pipeline path (`crates/html-pipeline/src/parser/html.rs`, `resolve_styles`):

- `resource_entries`, the program key and `get_or_prepare` (`program.rs`) each scan linearly comparing full CSS strings.
- `ResourceStylesheet::css()` (`01-source/resource.rs`) re-serialises the parsed sheet only to produce that key.
- `stylesheet()` deep-clones the AST on a miss.

**Proposal.** Assign an id at parse and key on that; keep the compiled fragment with the stylesheet. This removes `parsed_entries` and `CachedParsedStylesheet`.

**Check first.** `resolve_font_urls` creates fresh copies for sheets with relative font URLs; they need a stable id (source id plus base) or they will miss on every document.

### Property dispatch

`apply_longhand` (`05-cascade/resolver/properties/mod.rs`) tries six family functions in sequence until one handles the property. `paint::apply` resolves the parent style before it knows whether it handles the property, and `box_model::apply` reads font size and direction up front. A single routing match is simpler and cheaper.

### Smaller items

- CSS-wide keywords are passed as `&'static str` and packed into bits by string match (`05-cascade/resolver/context.rs`, `wide_keywords.rs`). Use an enum.
- There are about three `Instant::now()` pairs per element in `mod.rs`, which is expensive through `web_time` on wasm. Time whole phases or gate the split.
- There are nine `style_document_with_*` permutations across `lib.rs` and `program.rs`, while the pipeline uses two calls (`get_or_compile_resources`, `resolve_document_with_program`).
- `LocalMatchCache` and `StyleSharingCache` each hand-roll the same chained-hash and seen-twice admission logic.
- `property_is_always_computable`, the `RendererPropertyId` list and the native alias table are hand-maintained outside `property_catalogue!`.

## Leave alone

- The bucketed selector index.
- The compiled fast matcher with its general fallback.
- Copy-on-write font/text groups in `WorkingStyle`.
- Style sharing.
- The `property_catalogue!` macro as the single source of property facts.

## Suggested order

1. Session ownership and the `SiblingIndex` split: mechanical, no behaviour change.
2. Fragment-indexed linking and stylesheet identity.
3. Prepared inline blocks.
4. Typed renderer-owned values.
5. Preparation-time validity, last, because items 3 and 4 make it tractable.


## Implementation status

The findings above describe the original implementation. The follow-up refactor now implements:

- Session-owned matcher/cascade scratch, in-place rule ordering, and a pseudo-match cache that keeps its target key with its rule buffers.
- Document-owned traversal and separately owned match caches, passed through explicit mutable borrows. Matching no longer uses `RefCell` or `OnceCell`. Recursive matching takes and returns reusable scratch without holding a cache borrow across recursion.
- Shared contextual candidate matching and extracted pseudo-style storage.
- Fragment-indexed linking without copied declaration tables or per-effective-rule shared ownership handles.
- Stable resource IDs, resource-owned compiled fragments, and bounded caches for font-URL and import variants. Initial preparation consumes the parsed stylesheet instead of cloning its AST. Explicit source serialization or subsequent source transformations can reconstruct the AST from the prepared fragment.
- Shared declaration preparation for rules, inline styles, and hints, including a sparse typed renderer-value pool. All use the same block/index declaration-event representation. The address-keyed spacing cache is removed; substitution caching uses declaration IDs. Selector-list caches use collision-checked structural IDs, and general-name caches use semantic string keys.
- Preparation-time eligibility and value-representation checks, including shorthand expansion and viewport-height requirements. Unsupported declarations are excluded before selection, preserving eligible lower-priority declarations.
- One winner-selection pass followed by dependency-ordered computation of winning longhands. Cascade retries, transactional scratch-style snapshots, the computability whitelist, and duplicate declaration flags are removed. Shorthand resets respect individual winning targets.
- Direct property-family dispatch, typed CSS-wide keywords, shared cache admission/indexing, and whole-pass timing on wasm. Renderer identities and native aliases live in the property catalogue.
- `StyleRequest` replaces the nine `style_document_with_*` permutations, with fresh compilation, cached compilation, and supplied-program resolution. The simple `style_document` convenience and resource-oriented program/resolution API remain. Callers of the removed wrappers must migrate to `StyleRequest`.

### Prepared values and supported-value policy

Native sizing, box lengths, font and line-height values, grid tracks/placements/areas, and fallible keyword mappings are converted into supported engine values during declaration preparation. Renderer-owned spacing, object position, and outline offset also retain prepared lengths instead of parser expressions. Shorthand components are prepared together; values produced by variable substitution go through the same conversion before application.

Deferred calculations retain font, root-font, line-height, viewport, and percentage dependencies. Layout still resolves the percentages and font metrics represented by the existing computed-style types. Size comparisons preserve their supported one-to-three operands. This does not introduce general nested-comparison support.

The independent `value_support` checker and the old fallible application conversions have been removed. Applying a prepared value has no failure result; unsupported syntax/representations are rejected at preparation. Missing viewport-height inputs are handled before application, and invalid substituted declarations use `unset`. Internal assertions cover prepared invariants and valid style-store handles, rather than serving as unsupported-CSS handling.

Contextual arithmetic saturates at finite representable values. Negative literals are rejected where required, while computed nonnegative scalar values clamp to zero. Expressions whose deferred font components cannot satisfy the existing storage constraints are conservatively rejected during preparation, rather than partially clamping their coefficients during application.

### Verification and remaining limits

Verification below predates the latest prepared-native-value conversion and does not validate it:

- Style all-target compilation succeeded.
- 375 existing style unit/integration tests passed. No new tests were added. Eight pinned WPT cascade tests and the full declarative CSS parsing sweep were excluded because the external WPT checkout was unavailable (`HTML_WPT_ROOT`).
- The broader pipeline check reached a layout error caused by a test-only `Range` import used by production code. The import restriction has been removed, but that fix has not been compiled again; pipeline compilation remains unverified.
- The latest conversion received source inspection, formatter parsing, and `git diff --check`. Compilation and tests were not run, in accordance with repository instructions. Existing grid-area checks were migrated to the preparation path; no additional test cases were introduced.

A release benchmark run with 11 samples recorded:

| Case | Median ms | Allocations | Allocated bytes |
| --- | ---: | ---: | ---: |
| Plain | 0.512 | 51 | 106,174 |
| Overrides | 8.457 | 8,101 | 1,414,996 |
| Substitution | 17.068 | 33,460 | 10,057,638 |
| Deep | 30.820 | 79,002 | 19,147,844 |

This run preceded the final grid-support, numeric-clamping, and preparation-validity adjustments. There is no comparable before-refactor baseline, so these measurements do not establish a speedup or characterize the exact final revision.

The architectural changes are implemented but the latest conversion is uncompiled and untested. Remaining verification includes style and pipeline compilation, rerunning the existing style tests, the nine external-fixture tests against the pinned WPT checkout, and a final benchmark with a comparable baseline. Concurrent changes elsewhere in the shared workspace mean earlier successful checks do not validate every current repository change.
