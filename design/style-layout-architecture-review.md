# Style → layout architecture review

Date: 2026-11-16
Scope: the seam between `crates/03-html-style`, `crates/html-style-model`, `crates/html-format`, `crates/html-shape` and `crates/04-html-layout` — how a style value travels from declaration to the number layout uses.

Read from source only. Nothing was compiled or run, and counts come from grep. Findings already covered by the other `design/*-review.md` files and by `crates/04-html-layout/*_SIMPLIFICATION.md` are not repeated here.

**The tree moved during the review.** The box builder files in `crates/html-format/src/box_builder/` were rewritten at 02:13 on 16 November, after items 3, 7 and 8 were written. A per-element `ElementClassifications` table now exists (see the status note under item 7). File and line references into `html-format` in items 3, 7 and 8 describe the earlier state and may no longer match.

## 1. Font-relative deferral doubles the value model (biggest lever)

`ex`/`ch`/`cap`/`rch`/`rcap`/`rlh` stay unresolved until shaping picks a face. Everything that can hold a length therefore exists twice:

- **Twin types:** `LengthPct`/`UsedLengthPct`, `PreferredSize`/`UsedPreferredSize`, `ComputedSizeComponent`/`UsedSizeComponent`, four `GridTrack*`/`UsedGridTrack*` pairs, plus used variants of outline, text decoration, object position and radii.
- **Twin views:** `StyleView` and `UsedStyleView` share 91 accessor names (`html-style-model/src/computed_style.rs:4399` and `:4885`).
- **Deferred fields:** six on `Font`, `line_height_x_height_px`, the sign-bit `FontRelativeLength`, and five coefficients in every `Calc` and `PreparedLength`.

The deferral is keyed more finely than it needs to be. The metrics cache key in `html-shape/src/execution.rs:329` uses only font-group fields plus language, so a used style is a pure function of `StyleIndices`. Despite that:

- `BoxFontMetrics` stores metrics per box in three strategies (`NotRequired`/`Dense`/`Indexed`), with a separate hash map for non-box styles.
- `PassContext::build` (`04-html-layout/src/engine/reader.rs:76`) then builds a `UsedStyleView` per box and dedups sizes and box models by `(indices, metric bits)`, and does it again for paint replay.

The deferral also decides which units authors can use where. `PreparedLength::unit` (`03-html-style/src/style/02-syntax/prepared_length.rs`) takes a `LengthMode` with six values, and accepts `ex`, `ch` and `cap` only in the modes whose computed storage has room for them: `ex` is accepted for a border width but not inside a `calc()` there; `ch` and `cap` are accepted for sizes and spacing but not for borders; `rex`, `ic` and `ric` are rejected everywhere. The storage types differ in what they can defer — `FontRelativeLength` holds only `ex`, `TextSpacing` four components, `Calc` five — so support is a property of the representation rather than of the feature.

Two more things follow the same key and belong in the same table:

- `PassContext` does not depend on layout constraints, yet `compute_layout` (`04-html-layout/src/layout.rs:17`) rebuilds it on every call: each relayout at a new width and each first-line refinement pass repeats the per-box style resolution.
- `ellipsis_glyphs` and `hyphen_glyphs` are hash maps keyed by box index (`html-shape/src/execution.rs:51`), although the glyph chosen is a property of the font, like the metrics.

**Recommendation.** Resolve once, at shaping, into a used-style store keyed by style handle, with one metrics entry per distinct font and language. Layout then sees only used types and plain field reads. `BoxFontMetrics`, `RequiredFontMetrics`, the `size_keys` dedup and the lazy wrapper accessors go away. `StyleView` stays for the pre-shaping stages. The store is built once per shaping and shared by every layout pass and paint replay.

**Not recommended.** Moving metrics into the cascade removes the computed half as well, but it ties the style cache to font availability and makes a reader font change force a re-cascade.

## 2. Three value forms and three property identities at apply time

In `03-html-style/src/style/05-cascade/resolver/properties/mod.rs`, a winning declaration can be applied through any of:

- `PreparedNativeValue`, which re-matches on `Property` to find its target field (`prepared_native.rs:426`);
- `PreparedRendererValue`, via `unparsed::apply`;
- the raw lightningcss `Property`, through a ~170-arm routing match (`:160–300`) into six family appliers returning `ApplyResult`.

There are also special cases ahead of the dispatch for `white-space`, `font`, `direction` and `background-image`. Identity is split across lightningcss `PropertyId`, `PropertyKey` and `RendererPropertyId`.

**Recommendation.** Finish the migration the style review started. Have preparation produce one `PreparedValue` for every supported longhand, keyed by `PropertyKey`, so apply is a single `match key`. The lightningcss AST then ends at preparation, and substituted values re-enter through the same prepare function. The routing match, family appliers, `ApplyResult` and `physical_side`'s `PropertyId` match all go.

## 3. Box facts are re-derived from style throughout layout

`Display` (22 variants) collapses to `LayoutMode` (8) in the box builder (`html-format/src/box_builder/classification.rs`), so layout keeps going back to style for what was lost:

- `establishes_formatting_context` takes seven arguments (`04-html-layout/src/block/margins.rs:272`) and, with the effective-overflow helpers, is referenced 18 times.
- Float checks appear in 16 files and `PositionMode::Absolute` checks in 21, across layout, format and pipeline.
- `has_floats` and `tracks_overflow_clips` are recomputed per pass.

**Recommendation.** Compute a small flag set once in the box builder and store it on `LayoutBox`: floated, absolutely positioned, establishes a formatting context, atomic inline, flex/grid item, clips. The document-level flags become an OR over boxes. This is mechanical and low risk.

Two further facts belong in the same record:

- **Used display.** `build_classified_element` (`html-format/src/box_builder/mod.rs:118`) derives the used display — replaced-element fixes, note suppression, root `contents` → `block`, blockified floated/absolute inlines — and then keeps only the `LayoutMode`. Layout's `establishes_formatting_context` reads `style.display()`, which is the computed value, so the two can disagree. Storing the used display on the box removes both the re-derivation and that gap.
- **Flow root.** `whitespace_context_root` / `flow_root` / `formatting_root` are referenced about 29 times across text preparation, shaping, layout and the pipeline, each walking the parent chain. A per-box flow-root index written by the builder makes these lookups.

## 4. Size resolution has no single owner

`UsedPreferredSize` is matched at about 240 sites in 24 files. `04-html-layout/src/boxes/constraints.rs` alone has eight `resolve_*` variants, and `flex_grid/style/conversion.rs` and `flex_grid/tracks.rs` carry further copies for Taffy.

**Recommendation.** One method that takes an optional percentage basis and returns definite, auto or an intrinsic keyword, so formatting contexts branch on three cases. Folding `Px`, `Percent` and `Calc` into one linear `{px, pct}` form would cut the variants further. Box edges are `f32` (`ResolvedBoxModel`) while layout is `f64`; pick one at this boundary.

## 5. Reader preferences are applied in several places

From what was read, they enter:

- as fields inside computed structs (`reader_font_weight`, `reader_hyphens`, `reader_word_spacing_factor`);
- as a full re-intern of the style table (`with_reader_overrides`, `computed_style.rs:3503`);
- in layout's `ParagraphSpacing`;
- as paint colours attached per accessor (`with_paint_colors`).

A single reader-overlay step that produces the store from item 1 would give these one home.

Traced further after the first pass, this is a finding rather than a lead. There are three implementations of the same policy in `html-style-model/src/computed_style.rs`:

- `ComputedStylesBuilder::apply_reader_overrides` (`:2811`) mutates the store in place at the end of the cascade. This is the production path.
- `ComputedStyles::with_reader_overrides` (`:3503`) builds a second store by re-interning every style, with `reintern_with_reader_overrides` and seven `reintern_*` helpers (`:3077–3456`) to carry store-bound string and size-expression IDs across. Its only callers are tests: `04-html-layout/src/inline/tests.rs`, the test module of `engine/paragraph_spacing.rs`, a resolver test and the model's own tests.
- `reader_paint_colors` (`computed_style/paint.rs:26`) resolves the paint part alone, for repaint without restyle. This is also production.

So roughly 450 lines of non-test code exist to serve tests, and those tests exercise reader behaviour through a path the application never takes. The two production paths share the small `apply_reader_*` helpers, which is the part worth keeping.

**Recommendation.** Move the layout and resolver tests onto the production path (style with overrides through `StyleRequest`), then delete `with_reader_overrides` and the `reintern_*` family.

## 6. The effective style of a character is merged in four places

A character's style is the owning box's style, overridden by a first-letter run (`FirstLetterStyles`), overridden again by a first-line range (`TextPaintRange`, produced by shaping refinement). There is no single table holding the result; each consumer merges the sources itself with its own `partition_point` / `segment` loop:

- shaping span planning (`html-shape/src/spans.rs:399`), first-letter only;
- inline token preparation (`04-html-layout/src/inline/01-prepare/text.rs:233`), first-line ranges then first-letter;
- first-line style range construction (`html-pipeline/src/stages.rs:146`);
- paint colour runs (`html-pipeline/src/stages/lifecycle.rs:1865`), first-letter then paint ranges.

The precedence is therefore written four times, and `PaintColorRun` is a fifth per-range table derived from the same inputs.

**Recommendation.** One sorted run table of `(character range, StyleIndices)` covering overrides from both pseudo-elements, with a single `segment(start, end)` lookup. Text preparation writes the first-letter runs; first-line refinement produces a new table by overlaying its ranges once. Shaping, token preparation and paint then read the same table, and the paint colour runs become a projection of it.

`::first-letter` is also selected twice. The box builder runs a `FirstLetterSelection` over source text to split off a floated first letter into its own box (`html-format/src/box_builder/construction_inline.rs`, `process_text_node`), and text preparation runs the same automaton again over collapsed text to produce the style overrides (`html-text/src/pseudo_styles.rs`, `prepare_first_letters`).

The rest of the text chain (`InlineContent` → shaping spans → inline tokens → fragments) was read at the type level and looked like distinct stages rather than duplicates: each holds data the previous one cannot. Minor: `InlineItemKind::Text` and `Marker` carry the same payload and are re-unified by `text_run()`, and the character ranges are named `glyphs` throughout although they index characters.

## 7. The box builder classifies each element several times

`ElementClassification::new` is called at about 14 sites and `display_for_element` at about 10 across `html-format/src/box_builder/`. A parent classifies its children to choose a construction path, and each child is classified again when built.

The look-ahead predicates are recursive subtree scans:

- `has_block_child` (`inline.rs:165`) calls `has_inline_splitting_block_descendant` and `has_table_internal_descendant`, both of which recurse through inline and `contents` descendants;
- `walk_split_inline` (`construction_inline.rs:21`) calls `has_inline_splitting_block_descendant` on each inline child and then recurses into it, so a nested inline subtree is rescanned once per level.

Cost therefore grows with inline nesting depth, and the same question is answered by code in three files.

**Recommendation.** One pass over the DOM before construction that fills a dense per-node table: used display, floated, absolute, and three bottom-up bits (contains an inline-splitting block, contains a table-internal box, has block-level pseudo). Construction reads the table. This is the natural producer of the box facts in item 3, and it follows the single-pass preference in `AGENTS.md`.

**Status.** The working tree now contains this table: `ElementClassifications` in `classification.rs` is filled by one explicit post-order walk and carries `has_block_child`, `splits_inline_descendant` and `table_internal_descendant`, and the flex and table builders read it. It was not written as part of this review and was not reviewed line by line. Two things from this item and item 3 remain open in that code: `display_for_element` still recomputes the display from style, and the classification is build-local, so the used display and the box facts are still not stored on `LayoutBox` for layout to read.

## 8. Elements and generated pseudo-elements take parallel paths

`::before` / `::after` boxes are built by functions that mirror the element ones: `pseudo_display` and `pseudo_is_block_level` beside `ElementClassification`, `walk_split_pseudo` beside `walk_split_inline`, `GeneratedEdge` in `block.rs`, `FlexInlineSource::{Node, GeneratedPseudo}` in the flex path, `GeneratedInlinePlan` beside `InlineFragmentPlan`. A rule about block-level detection or splitting has to be kept in step in both.

**Recommendation.** Give construction one box-source type (element, or pseudo of an element) with one classification, so each construction function has a single path and the pseudo case differs only in where content comes from. Do this after item 7, which supplies the shared classification.

## Style crate core

Items 9–13 come from reading the resolver (`05-cascade/resolver/mod.rs`, `runner.rs`, `specified.rs`), declaration preparation (`03-rules/prepared.rs`, `compiled.rs`) and the caches. Paths in this part are relative to `crates/03-html-style/src/style/`.

### 9. A declaration is an AST node plus seven side tables, read through a three-way match

`PreparedDeclarationBlock` (`03-rules/prepared.rs:116`) keeps, per declaration and alongside the lightningcss `Property`: a target range, the target keys, an expansion range, a pool of expanded longhand `Property` values, a renderer-value index, the renderer values, and a boxed slice of native values. Eligibility and viewport-height flags are packed into the target range, and shadow masks sit on the rule.

Three things follow from that shape:

- **Three block sources.** Stylesheet rules, inline styles and presentational hints each wrap the block differently, so `DeclarationEvent::view` (`specified.rs:142`) is a three-way match that looks up all six fields. Every accessor (`property`, `prepared_targets`, `prepared_longhands`, `renderer_value`, `native_values`, eligibility) rebuilds the whole view, and `apply_cascade` calls about eight of them per event.
- **Preparation is implemented twice.** `append_fragment_declarations` (`prepared.rs:218`) does it ahead of time. `resolve_declaration` (`runner.rs:308`) does it again at apply time for substituted values, into a different struct (`ResolvedDeclaration`) with its own flags (`owns_value`, `expanded`, `substituted`, `valid`). The two must agree on canonical parsing, expansion, native preparation and viewport-height detection.
- **Apply chooses between them per call.** `apply_selected_declaration` picks `resolved.native_values` or the prepared ones, `resolved.renderer_value` or the prepared one, and prepared or resolved longhands.

Further hops through the AST that the same change removes:

- **Tokens to text to AST.** `prepare_constant_value` (`03-rules/prepared.rs:331`) takes a renderer-owned property kept as tokens, serialises it to a CSS string, reparses it with lightningcss as the native property, and stores that `Property`, which is then prepared into a native value. `resolve_declaration` does the equivalent after substitution.
- **Hints build AST nodes per element.** `html_presentational_hints::collect` constructs lightningcss `Property` values, including token lists for renderer keywords, and `hint_preparation.append` then prepares them. Both happen for every element that reaches the cascade.
- **Targets are found by name.** `append_targets` (`02-syntax/targets.rs:51`) maps a lightningcss longhand to a `PropertyKey` through `PropertyKey::from_name(id.name())`, a string match.
- **Substitution goes through text.** After `var()` substitution, `parse_property_tokens` (`05-cascade/resolver/custom_properties.rs:588`) serialises the token list to a CSS string and has lightningcss parse it as the property, once per declaration and variable environment (cached up to 256 entries).
- **Custom properties have their own selection loop.** `cascade_custom_properties` (`custom_properties.rs:314`) walks the events in reverse with rollbacks and wide keywords, as `select_specified_values` does for ordinary properties. The two loops share the rollback map but not the code.
- **The catalogue already knows the destination.** `property_catalogue!` lists the computed field for every key, and `copy_view` (`05-cascade/resolver/wide_keywords.rs:84`) uses it to implement `inherit`, `initial` and `unset`. Value application does not use it: `PreparedNativeValue::apply` matches on the `Property` variant again to choose the field.

**Recommendation.** This is the storage side of item 2. One `PreparedDeclaration` holding its target keys, one prepared value per longhand and its flags replaces the property, the longhand pool, the renderer index and the native slice. All three sources then expose a plain slice of these, an event is a reference to one, and substitution calls the same prepare function to produce an owned one. `ResolvedDeclaration` and the three-way view disappear. Hints emit prepared values directly, and setting a value goes through the catalogue's key-to-field mapping, the same way copying one already does.

### 10. Element and pseudo-element styles are orchestrated separately

`compute_style_for_dom_element` (`mod.rs:535`) and `compute_pseudo_style_for_dom_element` (`mod.rs:432`) run the same sequence — match, order, build events, cascade custom properties, apply — with the element version adding caches. Related duplication:

- The context-free specificity loop at `mod.rs:601` repeats the candidate loop inside `RuleMatcher::match_rule` (`mod.rs:273`) with a different predicate.
- `store_pseudo_styles` (`mod.rs:321`) cascades `::before` a second time with the first-line style as parent, and `::first-letter` a second time with that result as parent, to produce the before-first-letter style.

**Recommendation.** One `cascade_target(target, parent, declarations)` function used for elements and all pseudo targets, with caching as an optional wrapper around it. Fold the context-free pass into `match_rule`. The repeated pseudo cascades are inherent to the inheritance chain but should reuse the matched-rule plan explicitly rather than relying on `CachedRuleMatches` remembering the last target.

### 11. Selectors have three representations and two matchers

A selector exists as the lightningcss AST (kept in the `StyleRule`), as a compiled program of `FastTest`/`Compound` (`compiled.rs:480`), and as `BoundTest` after binding to a document (`04-matching/compiled.rs`). The compiled matcher returns `Option<bool>`, and `None` falls back to the AST matcher in `04-matching/dom.rs`. Callers choose among six entry points: `matches`, `matches_indexed`, `matches_indexed_in_traversal`, `selector_matches_dom_node`, `selector_matches_dom_node_in_scope` and `selector_matches_indexed_in_scope`.

The split itself is reasonable: the compiled form covers tag, id, class, attribute and simple pseudo-class tests with parent, ancestor and previous-sibling relations, and the AST matcher handles `:has()`, `:nth-*`, complex `:is()` and namespaces.

**Recommendation.** Keep both engines, but give the resolver one entry point that takes the prepared selector, the node and the traversal context and returns `bool`, with the fallback inside it. Extending the compiled program until the AST matcher can be deleted would remove about 900 lines, but it is a larger and riskier change and should wait for a measured need.

### 12. Five caches, two of them sharing styles under separate rules

One element can pass through `LocalMatchCache`, `StyleSharingCache`, `CachedRuleMatches`, `CustomCascadeCache` and `SubstitutionCache`, plus the fully-shadowed-rule pruning in `specified.rs:285`. Specific problems:

- **Two style-sharing mechanisms.** `LocalMatchCache` holds an `early_style` that is reused before matching, and `StyleSharingCache` reuses after matching. Each has its own eligibility rule.
- **A hand-maintained attribute list.** `element_is_shareable` (`style_sharing.rs:53`) lists 28 attributes that are "consumed outside selector matching". That list restates what `html_presentational_hints.rs` reads. Adding a hint attribute without updating the list would share a style that should differ.
- **Address identity remains.** `CustomCascadeCache` keys declarations by `property_address` (`custom_properties.rs:191`) and has its own hash-bucket chain, although `style-architecture-review.md` reports address keys replaced by declaration IDs and cache indexing shared through `CacheIndex`.

**Recommendation.** Derive the out-of-band attribute set from the hints module so there is one source. Key the custom cascade cache by `DeclarationId` and move it onto `CacheIndex`. Whether early sharing still pays for itself next to post-match sharing is a measurement question; nothing was measured here.

### 13. Reader policy runs inside the cascade

This confirms part of item 5. `enforce_reader_minimum` (`runner.rs:104`) is called after every prerequisite declaration and again at the end of the phase, and it zeroes the six deferred font-size fields by hand. `font_size_scale` is derived from reader overrides into `ResolutionContext` (`mod.rs:85`), and `apply_reader_overrides` runs on the builder at the end (`mod.rs:225`). Together with `with_reader_overrides` on the finished store, reader preferences are applied before, during and after the cascade.

**Recommendation.** Apply the minimum once, after the prerequisite phase, and keep it with the other reader adjustments in the single overlay step proposed in item 5. The scale has to stay in the cascade because relative sizes inherit it.

## Pipeline and cross-cutting

### 14. Document preparation is written three times, behind about seventeen entry points

The sequence validate styles → prepare MathML → build the box tree → prepare text → build indexes → bind paint appears in `FormattingDocument::try_new_with_note_flow` plus `prepare_text` (`html-pipeline/src/stages/lifecycle.rs:136` and `:180`), and again in `reprepare_and_layout_candidate` (`:655`). The second copy also applies reader paint overrides to the palette; the first does not. `restore_responsive_preparation` (`:1708`) is a third variant: it revalidates, re-prepares MathML, rebuilds the palette and assembles a borrowed `PreparedInputs` by hand before shaping.

Three structs carry the same derived inputs: `FormattingDocument`, `PreparedInputs` and `ResponsivePreparation`. `ResponsivePreparation::exchange` and the end of `reprepare_and_layout_candidate` each swap them field by field, eight `replace` calls apiece.

On top of that there are about seventeen ways to shape, lay out or repaint across the three document types:

- `PreparedDocument`: `shape`, `shape_borrowed`, `shape_and_layout`, `shape_and_layout_with_metrics_and_shaper`, its `_timed` twin, `reprepare_and_layout`;
- `ShapedDocument`: `layout`;
- `LaidOutDocument`: `relayout`, `relayout_from`, `relayout_with_paint_from`, `repaint_from`, `repaint_with_reader_overrides`, `update_with_reader_overrides`, `reshape_and_layout_with_metrics_and_shaper`, `reprepare_and_layout`, `reprepare_retaining_previous`, `restore_responsive_preparation`.

`PipelineSession::update_inner` (`html-pipeline/src/session.rs:524`) chooses among them with a stage computation followed by nested conditions that re-derive `parsed_reused`, `prepared_reused` and `shaped_reused` from that stage. `layout-architecture-review.md` reports the layout entry points consolidated to two; that holds for the request type, not for the number of paths. `pipeline-architecture-review.md` finding 1 covers the stage keys.

**Recommendation.** One `prepare(document, styles, note_flow, fonts, overrides)` function returning the derived inputs, called from both places. Keep the document outside that struct, so the responsive swap is a single `mem::swap` and the second struct is not needed. Fold the timed variant into its untimed one by returning timings from a shared inner function.

### 15. Storage forms chosen at run time

Several structures have two or three internal layouts selected by a size heuristic, with every accessor matching on the form:

- `BoxFontMetrics`: `NotRequired` / `Dense` / `Indexed`, plus `RequiredFontMetrics`;
- `NormalizationMap` in `html-text/src/normalization.rs:62`: `Characters` / `Runs`;
- first-letter overrides: a dense slice during preparation and sparse runs afterwards, unified by the `StyleOverrides` trait;
- text fragments in `html-engine-model/src/text_fragments.rs`: packed, compact and owned forms;
- `Storage<'a, T>` (owned or borrowed) around every prepared input in the pipeline, and `TokenBuffer` (a `Cow`) in inline layout;
- `LinePlacements` in `04-html-layout/src/inline/fragments.rs`: `Full`, `GlyphX`, `Sequential` and `Positioned`, with eight fragment builders in `inline/05-emit/fragments/mod.rs` covering the combinations of sequential or positioned, text or box.

Each one doubles the paths through its accessors. Some will be justified by measured memory savings on large books; the review found no record of which.

**Recommendation.** For each, keep the dual form only where a benchmark shows it matters, and otherwise keep the sparse form alone. Item 1 removes the first entry outright.

### 16. Smaller observations

- `LayoutMode::TableCell` dispatches to the same `layout_flow_children` as `Block` (`04-html-layout/src/boxes/02-contents.rs:118`); the variant exists to carry `colspan` and `rowspan`. A side table for spans would let cells be ordinary block containers. `Inline` and `Anonymous` also share a dispatch arm; the measurement notes record that pair as deliberately kept.
- `LayoutInputs` has 17 fields and `LayoutReadContext::new` forwards nine of them into `InlineReader::new` positionally.
- `tools/pipeline-bench/results/` holds full copies of crate sources from earlier benchmark runs. They are not part of the build, but they match every repository-wide search.

## Layout internals

Items 17–22 come from reading layout's sizing, block flow, inline stages, table, flex/grid, finalization and paint code. Paths are relative to `crates/04-html-layout/src/`. The `*_SIMPLIFICATION.md` files in that crate cover per-module cleanups; these items are the cross-module patterns.

### 17. A box's sizing is resolved several times per pass

`resolve_box_sizing` is described as running once per box ("sizing is never guessed or repeated by the parent", `boxes/01-sizing.rs:83`). In block flow it is called for the same box from:

- margin analysis: `compute_margin_profile` (`block/margins.rs:336`) resolves the sizing of every box with an open edge to get the basis for its children, and recurses down the leading and trailing child chains;
- `prepare_child` (`block/01-prepare_child.rs`), for the leading formatting-context candidate, followed by an isolated layout of it;
- `layout_child` (`block/02-layout_child.rs`), the real one;
- `fit_beside_floats`, again at each narrower width tried.

Margin analysis is cached per box and basis; the sizing it computed is not. `used_after_margin` then resolves min and max height a further time through a different route, from style rather than from the resolved sizing.

**Recommendation.** Cache `ResolvedBoxSizing` per box and request for the duration of a pass, next to the margin profile, and have `used_after_margin` take the resolved sizing. Margin analysis needs only the vertical part, so a narrower function for it is an alternative.

### 18. Resolved sizes are re-encoded as style values

`resolve_width_constraints` (`boxes/01-sizing.rs:488`) turns `min-content`, `max-content`, `fit-content` and `stretch` into pixel numbers and then wraps them back into `PreferredSize::Px((content + inset) as f32)` so that the common path can subtract the inset again. The value passes through `f32` and the box-sizing conversion is applied and undone. `ResolvedHorizontalSizing` keeps `preferred`, `min` and `max` as `PreferredSize` for later consumers to resolve again.

The same conversion between content-box and border-box sizes is written out separately in at least four places: `boxes/constraints.rs`, `AbsoluteAxis::stretched_border_size` (`boxes/absolute_positioning.rs`), `fixed_basis_auto_column_height` (`flex_grid/01-prepare.rs`) and `used_after_margin` (`block/margins.rs`). Auto-margin distribution is likewise written in `resolve_margin_left_from_edges`, `block_inline_alignment_offset`, `AbsoluteAxis::start_margin` and the table's `align_in_block_flow`.

**Recommendation.** This is the concrete case for item 4. A resolved size type that is already in content-box pixels, or auto, or an intrinsic keyword, removes the re-encoding, and one axis helper for "distribute free space to auto margins" serves block, absolute and table placement.

### 19. Generic box code and the table context each know about the other

Generic sizing and finishing test for tables repeatedly: `is_table_cell_box` to blank the authored widths and drop margins, `is_table_box || display is table` to pick the collapsed box model, `table_width_is_minimum` and `table_height_is_minimum` in `resolve_used_border_size`, a decoration-rect fix-up in `finish_resolved_box`, and `Display::Table` in block flow to realign afterwards. In the other direction `resolve_table_input` (`table/00-layout.rs:157`) recomputes the content width that generic sizing has just produced and reports `size_is_final` so the generic code will not touch it.

**Recommendation.** Let a formatting context declare how the generic stage should treat it — whether authored sizes are minimums, whether it owns its final size, whether it supplies its own borders — as a small record returned with the layout mode, instead of the generic code testing for tables by name.

### 20. First-line and reader policy are threaded through every inline stage

`::first-line` enters inline layout by five side channels: `text_style_ranges` in token preparation, `pseudo_line_heights` in paragraph preparation, `apply_first_line_pseudo_styles` rewriting token metrics into a per-line `LineAdjustment`, `first_line_color` at emission, and the pipeline's refinement loop (`html-pipeline/src/stages.rs:264`), which reshapes the affected ranges and re-runs layout of the whole document until the ranges stop changing, up to 32 passes.

Reader and book-mode policy is tested at about 27 sites in 16 files: `book_optimized_text` in token preparation, breaker selection, line measurement, hyphenation, table width fitting and both inline caches; reader paragraph splitting in `prose_separators`; reader indent in `prepare_box_input`; image fitting in sizing and the engine.

**Recommendation.** For first-line, item 6's single style-run table removes three of the five channels. The whole-document relayout is the expensive part; restricting the second pass to paragraphs whose first line changed needs measurement before it is worth doing. For reader policy, resolve it once into a per-pass policy record (breaker, fitting, splitting, indent) so stages read decisions rather than re-deriving them from constraints.

### 21. Finalization is nine whole-document passes, and paint ordering re-walks ancestors

`finalize` (`finalization/mod.rs:24`) runs publish-absolute, two line sorts, glyph-to-line indices, inline box geometry, image fragments by line, pagination groups, float keeps, ownership and overflow clips, each over all lines or all boxes. Paint replay follows with its own passes. `paint_order_key` (`paint/ordering.rs:14`) is called once per line and per decoration; each call walks to the root, re-derives stacking facts from style for every ancestor and allocates a boxed key.

**Recommendation.** The paint key of a box depends only on its ancestors, so compute it once per box top-down (parent key plus the box's own group) and have lines and decorations look it up. With the box facts of item 3 the stacking group is a stored field. Folding the finalization passes together is possible where they iterate the same collection in the same order, but only worth doing with timings.

### 22. Flex and grid carry a third style form and a list of solver workarounds

Each flex or grid layout, including every probe, converts the container and every item into a `taffy::Style` (`flex_grid/style.rs`, 678 lines), which allocates for grid tracks. Around the solver sit named workarounds: `compatible_auto_column_height`, `rerun_indefinite_percentage_rows`, `floor_fixed_grid_column_maxima`, `grid_percentage_width_needs_intrinsic_track_workaround`, stretch-equivalence tests, and separate intrinsic-width code in `flex_grid/intrinsic.rs` that does not go through the solver.

This does not change the earlier advice to keep Taffy. It does mean the boundary is the most expensive in the crate to maintain: about 3,000 lines adapt to a solver that then needs correcting in specific cases.

**Recommendation.** Keep a short list, in the module, of which workarounds exist and what upstream behaviour each compensates for, so they can be deleted when the dependency changes. Cache the converted item style per box within a pass where the inputs repeat between a probe and the final layout.

### 26. Formatting-context dispatch is written once per question

The match over `LayoutMode` that selects a formatting context appears in `dispatch_contents` for layout (`boxes/02-contents.rs:99`), in `calculate_content_widths` for intrinsic widths (`measurement/intrinsic.rs:152`), in `compute_margin_profile` for margin analysis, in `ParagraphSpacing` for prose detection, and in the baseline helpers of `boxes/03-finish.rs`. `calculate_outer_widths` repeats the table-cell margin rule and the collapsed-table box model from sizing, and has its own size resolver (`intrinsic_value`).

Replaced content has no mode of its own. `build_replaced_content` makes an image a block box whose inline children are one `Image` item owned by that same box; `LayoutMode` still carries a commented-out `Replaced` variant. Layout then sizes the image twice: generic sizing computes `replaced_size`, and inline token preparation has a `self_owned_replaced` branch (`inline/01-prepare/items.rs`) that builds a token from the width and height it was handed.

**Recommendation.** Give each formatting context one descriptor — layout, intrinsic widths, margin behaviour, and the sizing policy of item 19 — and dispatch on the mode once to obtain it. Add the `Replaced` mode so an image is sized in one place and never enters inline layout as its own child.

### 27. The two line breakers encode trailing-space and hanging rules separately

`KpSegmentPlan::build` (`inline/03-compose/wrapping/mod.rs:62`) and the greedy breaker (`wrapping/greedy.rs:54`) each decide which spaces collapse at a line end, where a discretionary break may fall and how hanging punctuation changes the limit. A third path composes line by line around floats. A testing-only assertion in `wrapping/optimal.rs` checks that the indexed tracking capacity equals the measured one, which shows the cost of keeping parallel encodings in step.

**Recommendation.** The Knuth–Plass breakpoint list already is a width-independent description of every legal break. Where a plan exists, the greedy breaker can take first-fit over the same list instead of re-deriving boundaries. Whether building the plan is affordable on the greedy-only path needs measuring, so treat this as a candidate, not a conclusion.

### 28. Text preparation rewrites the character buffer in separate passes

The box builder pushes raw source text. `collapse_whitespace` (`html-text/src/normalization.rs:213`) then builds new character, offset and wrap buffers and remaps every item; `transform_text` builds them again; `line_breaks::resolve` makes a third pass. `transform_text` allocates full-length `transforms` and `languages` vectors, and first-letter overrides are a full-length `Vec<Option<StyleIndices>>` until compacted into runs, although a first letter is a handful of characters per block.

**Recommendation.** Produce first-letter overrides as sparse runs from the start. Collapsing and case conversion can share one rewrite: the first-letter automaton consumes collapsed characters in order, so it can run inside the collapsing loop and feed the transform for the same character. This follows the single-pass preference in `AGENTS.md`; the saving is transient memory and two buffer rebuilds per preparation.

### 29. Paint colours are a fourth per-box style table

`PaintStyles` (`html-pipeline/src/stages/paint.rs`) keeps a `StylePaintColors` per box, two maps from style handle to colours (plain and linked), and per-box root and link flags found by walking each element's ancestors. Layout already holds a used-style view, a box-model index and a size index per box.

**Recommendation.** With item 3, "is root" and "is inside a link" are box facts. With item 1, colours are a column of the used-style store keyed by style handle and those two flags, and the per-box array is unnecessary.

### 30. Layout measures by running layout, and nothing caches a box's result

Whenever a size depends on content, the engine lays the subtree out into temporary output and reads the size (`measurement/isolated.rs`). There are about twenty such call sites:

- an atomic inline box is measured during token preparation (`inline/01-prepare/items.rs:601`) and laid out again at emission (`inline/05-emit/output.rs:299`);
- a formatting-context root beside floats is measured once, then once more per candidate position, then laid out (`block/float_fitting.rs`);
- a table cell whose height depends on its row is measured and later laid out again (`PreparedCellLayout::Relayout`, `table/04-rows.rs`);
- flex and grid items are measured through the solver's callback and for intrinsic block sizes (`flex_grid/01-prepare.rs`, `container.rs`, `measurement.rs`);
- the leading formatting-context candidate in block flow is measured before its parent's margins are decided (`block/01-prepare_child.rs`).

A probe of a box runs the probes inside it, and the final layout runs them again, so the work for a box can double with each level of nesting of these constructs. The inline paragraph cache and the flex/grid measurement cache absorb part of this; there is no cache of a box's layout result keyed by box and request. The measurement notes list such a cache as needing a separate performance investigation.

Atomic inline sizing in `create_atomic_box_token` is also one more copy of shrink-to-fit and min/max resolution (`resolve_border_width`), beside those in item 18.

**Recommendation.** Measure before designing: count probes per box on a table-heavy and a float-heavy document. If nesting shows up, a per-pass cache of `(box, request) → size, baselines` for probes is the smallest change, because probes discard their output anyway.

### 31. Smaller layout observations

- `cell_height_dependency` (`table/04-rows.rs:166`) scans each cell's descendants for percentage heights, and `FlowParticipation` (`block/margins.rs:59`) is derived from float and position on each use. Both are box facts in the sense of item 3.
- `layout_table_row` divides the width equally among cells. Table layout places cells through the grid, so this path runs only if a row is laid out outside a table; whether that can happen after anonymous-table fix-up was not established.

- `box_uses_ahem` (`engine/reader.rs`) compares the font family against "Ahem" for every text run prepared. A conformance-test font is special-cased on the production path.
- Pending absolutely positioned boxes sit in one queue for the whole document; `take_for` (`boxes/absolute_positioning.rs`) scans it linearly and removes from the middle for each containing block.
- Both inline caches list their invalidation inputs by hand (`ParagraphDependencies`, `PreparedInlinePlanKey`). A new input that affects line breaking has to be added to both.

## Style crate, second pass

### 23. Whether a declaration is usable is decided by overlapping predicates

The same question is answered in `02-syntax/` by:

- `ast_validation::property_is_valid`;
- `declaration_is_renderer_eligible` (`capabilities.rs:32`), used by preparation;
- `parsed_declaration_capability` (`capabilities.rs:103`), used by the public query and `@supports`;
- `declaration_syntax_impl` (`validation.rs:61`);
- `renderer_owned::tokens_are_supported` beside `PreparedRendererValue::parse`;
- `PreparedNativeValue::prepare`, which also returns an error for unsupported values.

The list of unsupported values — display, list style, text transform, text decoration, outline, `position`, `visibility: collapse` — appears twice in `capabilities.rs` in the same order, and unsupported display values are rejected a third time inside `PreparedNativeValue::prepare`. Eligibility for a renderer-owned alias serialises the tokens and reparses them, which `prepare_constant_value` then does again. `style-architecture-review.md` reports the independent support checker removed; these remain.

**Recommendation.** Make "prepare succeeded" the definition. Preparation returns the reason on failure, and the public capability query and `@supports` call it. The parallel predicates and their duplicated lists go.

### 24. A stylesheet moves between four forms, and text is the cache key

`ResourceStylesheet` (`01-source/resource.rs`) holds a parsed sheet behind a mutex, hands it to `PreparedStylesheetFragment::compile` on first use, and afterwards answers requests for the sheet by cloning the fragment's skeleton and copying the hoisted style rules back in (`PreparedStylesheetFragment::stylesheet`, `03-rules/prepared.rs:159`). `css()` serialises it to text again. Font-URL and import variants each rebuild from that reconstruction.

Lookups by text are linear: `StyleProgramCache::source` compares the full CSS string against up to 64 entries, and the inline style cache compares against up to 256 with a `VecDeque` that removes from the middle on every hit (`01-source/inline.rs:13`). A document with more than 256 distinct inline styles evicts continuously.

**Recommendation.** Once declarations are prepared values (item 9), the fragment needs only selectors and the at-rule skeleton, so nothing has to be taken out and put back. Key the text caches by hash with a verified compare. `pipeline-architecture-review.md` finding 5 covers the stylesheet caches as a group; this adds the inline one.

### 25. Matching carries its context twice and has optional paths

Further to item 11:

- "Does an ancestor have this tag, id or class" is held both as a bloom `AncestorFilter` per traversal frame and as an exact counted map (`Facts`, `04-matching/traversal.rs:49`).
- `SummaryTest` has two constructors, one from the compiled test and one from AST components, because both matchers ask the traversal.
- Both matchers take the traversal context as an `Option`. The paths without it serve `@scope` root matching and tests.
- `local_matching.rs` is a third consumer of the selector AST (`sibling_invariant`), after the compiler and the AST matcher.
- `SiblingIndex` is the match context; the name no longer describes it.

**Recommendation.** Make the traversal context mandatory so each matcher has one path, and rename `SiblingIndex`. Have the selector compiler record the facts `sibling_invariant` needs on `PreparedSelector`, so the cache does not read the AST.

### 32. Smaller style observations

- `@supports` conditions and selector validity are evaluated during linking (`RuleWalk::visit`, `03-rules/prepared.rs`), once per media environment, although neither depends on the environment. They can be decided when the fragment is compiled.
- There are two public ways to style a document: `StyleRequest`, which owns the document, and `get_or_compile_resources` with `resolve_document_with_program`, which the pipeline uses. Tests and the `style_document` convenience use the first.

## Coverage

Bodies read, in non-test code:

- **Style model:** the computed-style types, store, builder, both views, reader-override and paint-colour code.
- **Style crate:** resolver orchestration, winner selection, the cascade runner, property application, prepared native values and lengths, wide keywords, declaration preparation, rule linking, capability and validation predicates, inline and resource stylesheet sources, the program cache, the local match cache, the custom-property cascade and substitution, presentational hint collection, the selector compiler, the general matcher's entry points and traversal facts.
- **Box builder:** classification, block and inline construction, the writer's box and item creation, flex construction. The builder changed during the review; inline construction was read in its later state.
- **Text and shaping:** whitespace collapsing, text transformation, first-letter and pseudo-style preparation, span planning, span shaping, document metrics, synthetic glyphs, first-line reshaping.
- **Layout:** the entry point and engine, `PassContext` and reader, box sizing, dispatch, finishing and absolute positioning, block flow, margin analysis, float layout and float fitting, replaced size resolution, the inline coordinator and caches, token preparation including atomic boxes, paragraph preparation, composition drivers, float composition, line preparation, emission entry, the table coordinator, cell preparation and column measurement, flex/grid preparation and container, most of the Taffy item-style conversion, intrinsic width measurement, finalization geometry, ordering and overflow, paint replay and paint ordering.
- **Pipeline:** preparation, shaping and layout entry paths, the laid-out document's relayout, reshape, repaint and responsive-restore paths, the first-line refinement loop, paint style binding, the session's update and restyle paths, style resolution and stylesheet expansion.

Outlined only (signatures and structure, not bodies): the Knuth–Plass solver, line measurement, alignment and bidi placement, fragment builders, table grid construction, row sizing and column distribution, collapsed borders and backgrounds, grid tracks and flex item sizing, the Taffy tree adapter, pagination and ownership in finalization, paint emitters, the float band structure, paragraph spacing, custom-property dependency ordering, the style-sharing cache body, grid and renderer-owned value parsers, the media environment, and Unicode line breaking. These are leaf algorithms; the paths into and out of each were read.

Not read: the pipeline's output stages (addressing, search, pagination projection, export), the DOM and parser crates, `mathema`, the render core and the platform adapters. The first three are covered by `pipeline-architecture-review.md` and `pipeline-core-architecture-review.md`.

The implementation status recorded in the earlier review documents was checked only where items 12, 14 and 23 contradict it. The `*_SIMPLIFICATION.md` files in `crates/04-html-layout` were read for status but not re-verified.

## Suggested order

Mechanical, low risk:

1. Finish the classification table in the box builder (item 7, in progress in the tree) and store the box facts, used display and flow root on `LayoutBox` (item 3).
2. Delete the test-only reader-override path (item 5).
3. Cache hygiene: derived attribute set, declaration-ID keys, hashed text caches (items 12 and 24).
4. One preparation function in the pipeline (item 14).
5. Per-box paint key computed top-down (item 21).

Contained refactors:

6. Single element/pseudo construction path (item 8).
7. Resolved size type and shared auto-margin helper (items 4 and 18), with a per-pass sizing cache (item 17).
8. Formatting-context descriptor and a `Replaced` mode (items 19 and 26).
9. One text style run table (items 6 and 20).
10. One matcher entry point with a mandatory context, and one cascade-target function (items 11, 25 and 10).
11. "Prepare succeeded" as the single support test (item 23).

Large changes, independent of each other:

12. Used-style store resolved once at shaping, including metrics, synthetic glyphs and paint colours (items 1 and 29).
13. One prepared declaration and value form (items 9 and 2).

After benchmarks can run:

14. Probe counts and, if warranted, a per-pass probe cache (item 30).
15. Reader overlay and per-pass policy record (items 5, 13 and 20).
16. Storage-form audit, shared breakpoints for both breakers, merged text rewrite (items 15, 27 and 28).

## Leave alone

- The interned split style groups.
- Preparation-time validity.
- Taffy as the flex/grid solver.
