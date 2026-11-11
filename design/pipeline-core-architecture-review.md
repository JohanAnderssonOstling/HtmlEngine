# Pipeline core architecture review: prepare, shape, contracts, DOM

10 November 2026. Focus: simplicity, restricted to changes that are
performance-neutral or faster.

This continues [pipeline-architecture-review.md](pipeline-architecture-review.md),
which covered orchestration and paint. It covers the viewport-independent
middle of the pipeline and the data it runs on: box construction
(`html-format`), text preparation (`html-text`), shaping (`html-shape`), the
stage contracts (`html-engine-model`), the DOM (`html-dom`) and the GPUI
shaper.

Method: source reading only. Nothing was compiled, run or measured. Coverage is
partial and uneven; see [Coverage](#coverage). Line numbers are approximate.

## Summary

This code is already tuned in the small (packed wrap policies, compact
normalization maps, reused scratch buffers), so there is less to remove than in
the orchestration layer. What remains falls into four groups:

- **Repeated work that one value would replace:** element classification in box
  construction, and prepare-stage indexes that are rebuilt on every reshape.
- **Plan-then-act passes** in box construction that allocate an intermediate
  list per container.
- **Guards for states that cannot occur in production:** glyph-ledger branch
  tracking, the geometry run free list, the DOM builder typestate.
- **Parallel access paths** to the same data: three DOM accessor families and
  the free-function layer already noted in the first review.

Two items (4.3 and 5.3) remove deliberate design choices and are marked as
decisions for you, not recommendations.

Implementation follow-up: the actionable core changes below are implemented.
Validation is limited to source inspection, Rust syntax parsing with `rustfmt`,
and whitespace checks. Compilation, tests and performance measurements were not
run. The original findings remain below as historical context; their old line
numbers are not current navigation references.

## 1. Box construction (`crates/html-format/src/box_builder`)

### 1.1 Each element is classified several times

**Implemented:** one `ElementClassification` carries style, display and flow flags through construction. Inline, flex and principal-table callers pass the classification into materialization; display-only queries avoid unrelated flow lookups.

- **Where:** `classification.rs:9-20, 93-137`, `inline.rs:55-102, 178-206`,
  `block.rs:83-157`, `mod.rs:113-165`.
- **What:** `display_for_element`, `is_floated`, `is_absolutely_positioned` and
  `is_clearing_break` each start from `styles.style_for_node`. For one block
  child they run in the parent's `has_block_child`, again in
  `plan_block_children`, again inside `starts_new_block_child_for_element`, and
  again in `build_box_for_element`. Inline children are classified in
  `process_inline_nodes` and again in `plan_inline_node`.
- **Simpler shape:** compute one small `Copy` classification per element
  (display, floated, absolute, clearing break) and pass it down. The
  predicate family (`starts_new_block_child`,
  `starts_new_block_child_for_element`, `starts_inline_split_child`) becomes
  methods on that value.
- **Performance:** fewer style lookups per element; no new storage.

### 1.2 Children are planned into lists, then materialized

**Implemented:** block children and split-inline fragments are emitted during traversal. Builder-owned scratch stacks retain only pending segments, with recursion offsets. Inline edge trimming uses the child iterator instead of collecting a node list.

- **Where:** `block.rs:32-81` (`Vec<BlockChildAction>`, each inline or table
  segment owning a `Vec<u32>`), `inline.rs:208-277` (events `Vec`, then parts
  `Vec` of segment `Vec`s), `construction_inline.rs:123` (children collected
  per inline container only to trim both ends).
- **What:** the action and event enums exist only to carry decisions across
  the plan/act boundary. Every block container allocates the list plus one
  vector per segment.
- **Simpler shape:** decide and emit in the same loop. Segments still need
  buffering because they are trimmed at both ends; one builder-owned scratch
  stack (push, record the start offset, truncate on flush) serves the whole
  recursion.
- **Performance:** removes per-container allocations. This matches the
  single-pass preference in `AGENTS.md`.

### 1.3 `display: contents` text styles use a whole-document pre-pass

**Implemented with a different mechanism:** removed the whole-document map and pre-pass. Text emission resolves its source parent's `display: contents` style directly. This also handles text deferred by anonymous-table and flex grouping, and scoped subtree builds, without relying on mutable traversal state.

- **Where:** `mod.rs:97-100, 263-290`; lookup at `construction_inline.rs:420`.
- **What:** `collect_contents_text_styles` walks every element before the
  build to fill a map that is empty unless the document uses
  `display: contents`. Every text node then does a hash lookup. Scoped note
  builds (`build_layout_inputs_from`) pay the full-document walk for one
  subtree.
- **Simpler shape:** keep the inherited style as a builder field, set while
  descending through a `contents` element. The traversal already passes
  through all three entry points (`InlineNodePlan::Contents` at
  `construction_inline.rs:256`, `EnterContents`/`ExitContents` at `:105-108`,
  `ContentsWithTableInternals` at `mod.rs:233`).
- **Performance:** one fewer DOM pass and no per-text-node lookup.

### 1.4 Node lists round-trip through raw `u32`

**Implemented:** pending DOM source lists carry `DomNodeId` throughout block, inline, flex and table construction. Box-index lists remain `u32`.

- **Where:** `mod.rs:168, 177`; `construction_inline.rs:123, 260, 293, 313,
  332, 340, 586`; `inline.rs:426, 433`.
- **What:** children are mapped to `DomNodeId::raw`, stored as `Vec<u32>`, and
  re-validated with `node_id_from_raw` at each use, with a `None` branch that
  cannot be taken.
- **Simpler shape:** carry `DomNodeId` (it is `Copy`).

## 2. Text preparation (`crates/html-text/src`)

### 2.1 The expanded wrap-policy view has no callers

**Resolved:** removed the expanded view, legacy accessor, invalidation calls,
and associated memory accounting; packed queries remain.

- **Where:** `content.rs:28-35, 54, 74-77, 144, 172-174, 211`.
- **What:** `whitespace_wrap_overrides()` and the lazily expanded
  `OnceLock<Vec<WhitespaceWrapOverride>>` behind it ("legacy slice API") have
  no call sites in the workspace, tests included.
- **Simpler shape:** delete the accessor, `as_slice`, the `expanded` field and
  the two `expanded.take()` calls. `InlineContent` shrinks.

### 2.2 Small duplication

**Resolved:** both sites now use `is_transparent_to_collapsing`; the existing
fast-path guard is preserved.

- **Where:** `normalization.rs:304-306` and `352-354`.
- **What:** the "transparent to collapsing" character test (bidi controls,
  soft hyphen, joiners, variation selectors) is written out twice.
- **Simpler shape:** one named function.

## 3. Shaping (`crates/html-shape`, `crates/html-pipeline/src/stages`)

### 3.1 Two prepare-stage indexes are rebuilt on every reshape

**Implemented:** `PreparedIndexes` owns word boundaries and link/anchor targets alongside prepared text. Initial preparation, re-preparation and scoped preparation build them; reshaping reuses them. Visibility reads the computed style, as the previous used-style query did.

- **Where:** `html-pipeline/src/stages/lifecycle.rs:1022-1027`;
  `stages/semantics.rs:66`; `stages_output/words.rs:14`.
- **What:** `shape_text` builds link and anchor targets
  (`collect_glyph_targets`) and the word index (`build_word_index`). Neither
  reads glyphs, advances or font metrics: they use the DOM, the box tree and
  the prepared characters. They are nevertheless stored in `SharedShapedData`
  and recomputed whenever fonts change, the backend generation changes, or
  glyphs are rehydrated.
- **Simpler shape:** build both once in preparation and keep them with
  `PreparedInputs`. `SharedShapedData` then holds only shaping results.
- **Check first:** the word index asks `metrics.used_style(..).visibility()`
  (`words.rs:27-31`). Visibility does not depend on font metrics, so the
  computed style should give the same answer; confirm before moving it.
- **Performance:** reshape gets cheaper; preparation, which can already run on
  a worker, does the work once.

### 3.2 `shape_document` is one 450-line function with a duplicated core

**Implemented:** `DocumentShape` names the results. Metrics, document spans and synthetic glyphs have separate functions; document and first-line shaping share `shape_span` and reusable character, glyph and string buffers.

- **Where:** `html-shape/src/execution.rs:55-506` and `638-795`.
- **What:** the function finds which boxes need metrics, resolves font
  metrics, shapes spans and shapes synthetic glyphs, then returns an unnamed
  4-tuple. Its per-span body (soft-hyphen filtering, scattered-glyph gather,
  backend call, geometry fallback, mirrored glyphs; `:341-455`) is repeated in
  `reshape_range_with_style` (`:690-793`), where it allocates a `String` and
  `Vec` per run instead of reusing scratch.
- **Simpler shape:** three functions (metrics, spans, synthetic glyphs)
  returning a named struct, and one span-shaping helper that takes the scratch
  buffers and serves both callers.
- **Performance:** neutral for document shaping; the first-line path stops
  allocating per run.

### 3.3 Span helpers repeat the same style lookup

**Implemented:** paint spans resolve styles through one method, and one traversal computes placement and tracking flags.

- **Where:** `html-shape/src/spans.rs:29-37, 48-51, 163-198, 618-620`.
- **What:** "override style, else box style" is written five times.
  `placement_required` and `has_tracking` each walk the paint spans and
  resolve letter spacing separately.
- **Simpler shape:** a `style()` method on `ShapingPaintSpan`, and one pass
  returning both flags.

## 4. Stage contracts (`crates/html-engine-model/src`)

### 4.1 The glyph ledger tracks branches for a rollback that is always scoped

**Implemented:** the append transaction privately retains the original ledger length. Rollback truncates registrations; checkpoint identities, branch history and discarded-prefix storage are removed. The store documents the exclusive append-only contract.

- **Where:** `shaping_contracts.rs:995-996, 1030-1034, 1156-1206`; the only
  engine user is `AppendShapingTransaction`
  (`html-pipeline/src/stages/lifecycle.rs:485-519`).
- **What:** `branch`, `discarded_prefixes` and the checkpoint identity check
  protect against rolling back to a stale checkpoint. The transaction holds
  the store by `&mut` from checkpoint to rollback, so no other checkpoint or
  rollback can interleave.
- **Simpler shape:** make the checkpoint a length private to the transaction;
  rollback truncates. `branch` and `discarded_prefixes` go away.
- **Check first:** the remaining `checkpoint()` callers are adapter tests
  (`adapters/html-gpui/src/text.rs:2638-2681`).

### 4.2 `ShapedTextGeometry` keeps a run free list that is never used

**Implemented:** runs use dense `Vec<TextGeometryRun>` storage without slot counts or a free list. Production first-line shaping appends disjoint ranges. The separate replacement API compacts dead runs when editing; its three `shaping_views.rs` callers are in tests.

- **Where:** `shaping_contracts.rs:88, 100-106, 144-163, 342-348`.
- **What:** `range_count` and `unused_runs` release a run when its last source
  range is replaced. The only production replacement
  (`html-shape/src/execution.rs:773`) writes disjoint ranges into a geometry
  created fresh for each first-line pass, so nothing is ever replaced twice.
  `replace_range` and `update_range` are test-only.
- **Simpler shape:** drop the count and the free list; `runs` becomes
  `Vec<TextGeometryRun>` without the `Option`.
- **Check first:** the three callers in `shaping_views.rs`, which I did not
  read.

### 4.3 Decision: two paint-ownership modes, one used in production

**Retained by decision:** both borrowed and retained paint resources remain supported, including their ownership checks.

- **Where:** `glyph_adapter.rs` (`NativePaintSource`),
  `shaping_contracts.rs:1003-1007, 1246-1251, 1321-1360`,
  `html-pipeline/src/session.rs:220`.
- **What:** a backend returns either borrowed handles or retained entries, and
  the store rejects mixing them at runtime (`MixedPaintOwnership`). The GPUI
  shaper only returns retained entries
  (`adapters/html-gpui/src/text.rs:1736, 1756, 1938, 1957`). Borrowed mode
  serves `ResourceShaper` (`tools/gpui-minimal`) and headless or test shapers.
  In retained mode the stored per-glyph paint handle equals the glyph ID
  (`shaping_contracts.rs:1342`).
- **If removed:** `GlyphPaintIdentity`, the ownership check and its error, the
  session's generation branch, and the per-glyph handle all go.
- **Why it is a decision:** `HANDOFF.md` records both modes as an intended
  contract. It would change the adapter API.

### 4.4 The free-function layer continues here

**Implemented:** shaping-contract, element-storage and render-query forwarding functions became methods or constructors, and workspace callers were migrated. The two lifecycle error variants remain because their live lifecycle APIs still use them; removing that API matrix belongs to finding 2 of the first review.

- **Where:** `shaping_contracts.rs:1642-1860` (about 25 functions such as
  `shaped_text_run_advances(&run)`), `html-dom`'s `storage::` module, and the
  `Render*` mirror structs in `html-pipeline/src/stages_output`.
- **What:** same pattern as finding 7 of the first review. No runtime effect.
- **Related:** `ShapeErrorKind::BorrowedDocument` and
  `FirstLineShaperRequired` exist only for the lifecycle API matrix (finding 2
  of the first review) and disappear with it.

## 5. DOM (`crates/html-dom/src/document/dom.rs`)

### 5.1 The builder typestate duplicates a runtime check

**Implemented:** one `DocumentBuilder` replaces the three typestates. `finish` validates root/topology, and the XML parser keeps one builder.

- **Where:** `dom.rs:101-328`; `crates/02-html-parse/src/xml.rs:103-105`.
- **What:** `NeedsRoot`, `Rootless` and `Rooted` each repeat
  `append_dom_element`, `append_dom_text` and `set_dom_root`. The root is
  validated again at finish (`validate_for_build`, `MissingRoot`,
  `dom.rs:745-750`). The XML parser holds two `Option`s to move one builder
  between states.
- **Simpler shape:** one builder; `finish` performs the check it already
  performs.

### 5.2 Three ways to read an element

**Implemented:** removed the forwarding `get_dom_*` family and element-storage helpers; callers use element/node reference methods. Existing class-iterator callers were migrated too.

- **Where:** `dom.rs:1021-1086` (`Document::get_dom_*`), `dom.rs:1192-1360`
  (`ElementRef` methods), `dom.rs:1362-1468` (`storage::` functions).
- **What:** the node-ID accessors forward to `ElementRef`. About 39 non-test
  call sites use them (`get_dom_tag` 17, `get_dom_parent` 13, `get_dom_id` 7,
  `get_dom_attr` 2). `get_dom_classes` boxes its iterator and has no callers.
- **Simpler shape:** keep `ElementRef`; migrate the call sites.

### 5.3 Decision: `DomNodeId` carries a document token

**Resolved:** `DomNodeId` now stores only a document-local `u32`, including
its element/text kind bit, with transparent representation (4 bytes). Callers
must pair IDs with their originating document; bounds and kind checks remain,
but foreign-document IDs are no longer rejected. Document-level lineage is
retained for computed-style compatibility checks. The findings below describe
the previous representation.

- **Where:** `dom.rs:28-32, 944-975, 1244-1251`.
- **What:** the ID is a `u32` plus a `u64` token, so 16 bytes. `element_ref`,
  `node_ref` and `text_ref` compare the token on every access, and child
  iteration re-attaches it to each raw child.
- **If reduced to a `u32`:** hash keys such as `FxHashMap<DomNodeId, _>`
  (`html-format` `mod.rs:72`, `lists.rs:18`) shrink to a quarter and the
  comparisons go.
- **Why it is a decision:** the token is a deliberate guard against using an
  ID with another document, documented with doctests.

### 5.4 Small: `element_by_id` discards the interned index

**Resolved:** lookup now compares interned IDs over elements only, preserving
the first matching element in insertion order.

- **Where:** `dom.rs:956-961`.
- **What:** it looks the ID up in the string table, drops the index, then
  string-compares every node, text nodes included.
- **Simpler shape:** compare the interned index over elements only.

## 6. GPUI shaper (`adapters/html-gpui/src/text.rs`)

**Not measured:** the cache-key observation below remains a profiling task; no performance claim or cache-key redesign is made.

No structural finding in the part I read. One thing to measure, not a
simplification:

- **Single-character cache key:** `GlyphKey` holds a full `Font`
  (`text.rs:171`). Building it for every one-character run
  (`text.rs:1902-1925`) scans the prepared-font list, clones the
  `PreparedFont`, and clones a `Font` again in `range_filtered_font`, all
  before the cache lookup.

## Looked at and left alone

These look complicated but I found no simpler form that is certain not to cost
time or memory:

- **`NormalizationMap`'s two variants** (`html-text/src/normalization.rs:55-146`):
  the dense form is the smaller one for heavily fragmented input.
- **Packed wrap policies** (`content.rs:31-78`), apart from 2.1.
- **`BoxFontMetrics` dense and indexed forms** (`shaping_contracts.rs:514-539`).
- **The font-metric cache's content key** (`execution.rs:151-181`): keying by
  font-style identity would be cheaper to hash but could add backend metric
  calls if equal styles are not deduplicated.
- **`ListItemOrdinals`' whole-document pre-pass**
  (`box_builder/lists.rs:17-52`): reversed lists need the count up front, and
  scoped note builds need numbering context from outside the subtree.
- **Multi-group `shape_line_geometry`** (`text.rs:1468-1675`): `shape_run`
  passes one span, but small-caps synthesis (`text/caps.rs:165`) needs several.
- **Caching each box's flow root:** `flow_root` is recomputed per inline item
  in several passes, but a side table costs memory per box.
- **`SpanBuilder` and the separate bidi split** (`spans.rs:76-136, 248-287`).

## Suggested order

1. **3.1 and 2.1:** small, isolated, and 3.1 speeds up every reshape.
2. **1.1, then 1.3 and 1.4:** mechanical within one crate.
3. **4.1 and 4.2:** deletions, after the two checks noted.
4. **3.2, 3.3, 5.1, 5.2, 5.4:** restructuring with no behaviour change.
5. **1.2:** the largest change in box construction.
6. **4.3 and 5.3:** only if you decide to drop those guarantees.

## Coverage

### Read

- **`html-format`:** `box_builder/mod.rs`, `writer.rs`, `inline.rs`,
  `construction_inline.rs`, `block.rs`, `classification.rs`, `generated.rs`,
  most of `lists.rs`.
- **`html-text`:** `lib.rs`, `content.rs`, `prepared_text.rs`,
  `normalization.rs` lines 1-560 and 690-787.
- **`html-shape`:** `execution.rs`, `spans.rs`, `features.rs`.
- **`html-pipeline`:** `stages/semantics.rs` (first 170 lines),
  `stages_output/words.rs`, the second half of `stages_output/text.rs`.
- **`html-engine-model`:** `lib.rs`, `shaping_contracts.rs` lines 1-440 and
  500-1400, the first 200 lines of `glyph_adapter.rs`.
- **`html-dom`:** node identity, builder signatures, lookup and `ElementRef`
  accessors in `document/dom.rs`.
- **`adapters/html-gpui/src/text.rs`:** lines 941-1050, 1180-1290, 1468-1970.

### Outlined only

- `html-dom/src/document/dom.rs` beyond the parts above.
- `html-resources/src/resources/images.rs` and
  `html/src/engine_resources.rs`. Image dimensions appear to live in several
  places (DOM attributes, the image pipeline, pending metrics, committed
  inputs, the published layout), which may be worth its own look.
- The rest of `adapters/html-gpui/src/text.rs`.

### Not read

- **`html-format`:** `table.rs`, `construction_table.rs`,
  `construction_flex.rs`, `list_marker.rs`.
- **`html-text`:** `line_breaks.rs`, `pseudo_styles.rs`, `first_letter.rs`,
  `spacing.rs`, `joining.rs`, `chunks.rs`, `source_offsets.rs`, and
  `normalization.rs` lines 560-690.
- **`html-engine-model`:** `glyph_metrics.rs`, `text_fragments.rs`,
  `caret_positions.rs`, `layout_tree.rs`, `paint_program.rs`, `bidi.rs`,
  `shaping_views.rs`, `text_placement.rs`, `word_index.rs`.
- **`html-dom`:** `storage.rs`, `columns.rs`, `cfi.rs`, `source/*`,
  `semantics.rs`.
- **`02-html-parse`:** the bodies of `xml.rs`, `encoding.rs`, `metadata.rs`,
  `text.rs`.
- **`html-pipeline`:** `stages/paint.rs`, `stages_output/boxes.rs`,
  `geometry.rs`.
- **Adapters:** `fonts.rs`, `text/caps.rs`, `html-fonts`.
