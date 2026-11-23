# Architecture findings: implementation record

Source: [style-layout-architecture-review.md](style-layout-architecture-review.md).
Line counts compare each patch with the working tree immediately before it,
excluding unrelated edits. Documentation counts are separate. No compilation,
tests, or benchmarks have been run; runtime verification remains outstanding.

| Finding | Work recorded | Added | Removed | Net |
| --- | --- | ---: | ---: | ---: |
| 7 | Cached box-builder classification; simplified inline traversal | — | — | -29 |
| 3 | Stored box facts and flow roots; removed repeated queries | — | — | -1 |
| 8 | Shared DOM/pseudo classification, splitting, and construction | 263 | 295 | -32 |
| 4 | Shared definite-size resolver and migrated callers; broader size representation and precision work remains | 53 | 102 | -49 |
| 5 | Removed test-only computed-style override API and re-interning helpers; migrated existing callers to production resolver/builder | 223 | 449 | -226 |
| 9 (declaration records) | Combine four parallel metadata arrays into PreparedDeclaration records; move append logic onto the block and encapsulate pooled access | 109 | 130 | -21 |
| 9 (event access) | Resolve one block per view; reuse views in selection/application and remove repeated per-field rule/inline/event accessors | 65 | 232 | -167 |
| 9 / 23 (shared preparation) | PreparedValue prepares static/substituted/queried values through one renderer/expansion/native path and shares viewport detection | 80 | 106 | -26 |
| 9 (effective application view) | Normalize static/substituted values before property dispatch; remove its ResolvedDeclaration dependency and redundant substitution-state bit | 42 | 60 | -18 |
| 10 | Fold local/context-free specificity matching into RuleMatcher with explicit local, element, and pseudo targets | 27 | 34 | -7 |
| 10 (cascade execution) | Shared element/pseudo cascade target; resolve custom-map IDs directly and remove result-enum plumbing | 118 | 158 | -40 |
| 10 (pseudo plans) | Match/order each pseudo once, pass its plan to inheritance variants, and remove CachedRuleMatches | 55 | 119 | -64 |
| 11 | One boolean selector entry point owns compact/AST dispatch for resolver candidates | 43 | 32 | +11 |
| 12 | Derived sharing eligibility from collected hints; declaration-ID cache keys; shared CacheIndex | 66 | 66 | 0 |
| 13 | Apply minimum font size once after prerequisites; retain line-height rebinding before remaining properties | 0 | 6 | -6 |
| 14 | Shared staged preparation, one active/cached prepared state, whole-state exchange, and one timed/untimed layout path | 241 | 250 | -9 |
| 14 (follow-up) | Session dispatch uses EarliestStage directly; removed redundant reuse flags and repeated source/cache checks | 13 | 17 | -4 |
| 25 | Compile sibling-sharing dependencies once; remove selector-AST traversal from the local sharing cache | 71 | 49 | +22 |
| 25 (follow-up) | Mandatory TraversalContext in both matchers; remove context-free sibling/relative paths and route scope matching through shared entry | 108 | 260 | -152 |
| 25 (ancestor facts) | Replace per-frame ancestor bloom masks with compiled requirements checked against shared exact traversal facts | 59 | 59 | 0 |
| 23 (native support) | Use native preparation for capability failures; remove duplicate display/position/visibility/text-transform checks and alias pre-parse | 6 | 69 | -63 |
| 23 (renderer support) | Replace token-support dispatch with prepared parsing; parse line counts directly from tokens; adapt existing allocation test call | 32 | 60 | -28 |
| 23 (shorthand/grid support) | Capability queries expand native shorthands; remove list/outline/decoration/grid predicates and validate remaining flags/names in preparation | 41 | 109 | -68 |
| 23 (renderer admission) | Shared renderer preparation for static, substituted, and queried values; remove eligibility predicate and duplicate parsing | 43 | 66 | -23 |
| 23 (query results) | Shared public/@supports result; remove redundant alias/fallback parsing; retain containment support metadata in prepared value | 29 | 46 | -17 |
| 23 (failure reasons) | PreparationError shared by renderer preparation and feature queries; remove separate raw syntax/capability predicates | 132 | 134 | -2 |
| 24 | Hashed inline/source text caches with equality verification and recency timestamps; stylesheet reconstruction awaits item 9 | 46 | 26 | +20 |

| 28 (sparse first-letter storage, production) | Emit sparse ranges directly; transform their boundaries and merge adjacent equal styles without full-length override buffers | 61 | 58 | +3 |
| 28 (existing test adaptations) | Update sparse consumers; remove obsolete dense-storage assertions and duplicate dense transformation | 15 | 66 | -51 |

| 28 (case metadata ranges) | Replace per-character transform/language arrays with sorted effective style ranges; reuse them for contextual casing | 57 | 27 | +30 |

| 19 / 26 (shared sizing policy) | Classify minimum-size and parent-owned inline-sizing rules once; retain the policy through final size resolution and reuse it in intrinsic measurement | 45 | 23 | +22 |

| 19 (table decoration ownership) | Publish the complete grid border rectangle in table placement; remove table-only rectangle expansion from generic box finishing | 9 | 21 | -12 |

| 19 (edge ownership policy) | Share collapsed-grid edge suppression across final sizing, intrinsic widths, and caption contributions; remove the table-specific box-model helper | 33 | 32 | +1 |

| 17 (trailing margin sizing) | Reuse resolved vertical constraints and bottom margin; remove repeat style resolution, reader dependency, and unused containing-width plumbing | 22 | 47 | -25 |

| 17 (per-pass sizing cache) | Cache the latest complete sizing request/result per box; share it with isolated measurements and reset at final-pass entry | 15 | 4 | +11 |

| 18 (numeric width limits) | Resolve content-box min/max once; reuse numeric bounds in box, ratio, table, and flex/grid sizing; remove the combined width resolver | 41 | 56 | -15 |

| 18 (intrinsic content widths) | Keep non-replaced intrinsic/stretch keyword widths as f64 content values, bypassing PreferredSize::Px and inset round trips | 48 | 42 | +6 |

| 18 (resolved auto-width fact) | Replace the retained preferred-size enum with the auto-width boolean actually used by finishing, table, and flex/grid consumers | 20 | 32 | -12 |

| 18 (numeric reader image width, production) | Return f64 content width and pass it directly into the existing replaced-size solver; preserve auto-width classification | 21 | 18 | +3 |
| 18 (existing replaced-size test calls) | Supply the absent numeric override at existing solver call sites; formatter wraps longer assertions | 20 | 8 | +12 |

| 18 (numeric replaced stretch, production) | Pass numeric preferred/min/max content overrides to the replaced solver; remove the remaining stretch-to-Px conversion | 56 | 49 | +7 |
| 18 (existing replaced-size test calls, follow-up) | Adapt the absent-override argument to the three-width contract | 8 | 8 | 0 |

| 18 (shared automatic margins) | Share free-space distribution across block/table and absolute positioning; preserve each overflow policy explicitly | 60 | 60 | 0 |

| 18 (shared axis conversions) | Use the shared definite-size resolver in absolute stretch bounds and fixed-basis flex-column measurement | 28 | 25 | +3 |

| 21 (shared paint ancestry) | Cache parent-first group paths by collection state, share paths for boxes without groups, and memoize nearest DOM ancestry | 71 | 26 | +45 |

| 21 (paint-path simplification) | Carry ownership through the parent-first rebuild; remove repeated target checks and the redundant local box argument | 6 | 14 | -8 |

| 31 (absolute-position queues, production) | Index deferred boxes by containing block and consume each FIFO directly; remove repeated queue scans and middle removals | 10 | 7 | +3 |
| 31 (existing queue-test adaptation) | Adapt setup to grouped queues while retaining source-order and allocation assertions | 8 | 9 | -1 |

| 31 (cell height dependency) | Cache each cell’s descendant height requirement for the immutable pass and reuse it across isolated probes | 26 | 6 | +20 |

| 32 (selector validity) | Validate style selectors once per fragment and retain the result in existing rule metadata | 13 | 14 | -1 |

| 32 (static conditions) | Compile supports conditions and scope-selector validity once; share nested-rule traversal | 33 | 19 | +14 |

| 32 (entry-point ownership) | Keep the owned convenience API beside program compilation/resolution; remove its forwarding layer and redundant input-copy mapping | 104 | 129 | -25 |

| 31 (inline policy dependencies) | Key both inline caches by TextCompositionPolicy instead of duplicated derived booleans; adapt existing fixtures | 14 | 14 | 0 |

| 31 (font metrics) | Remove Ahem family-name overrides; use selected-face normal height and space/ch advances | 9 | 43 | -34 |

| 31 (standalone row audit) | Confirmed scoped subtree roots can be table rows; retained the reachable fallback | 0 | 0 | 0 |
| 27 (leading boundaries) | Share leading whitespace/zero-width-break trimming between greedy selection and Knuth–Plass plan construction | 26 | 34 | -8 |

| 27 (edge-space predicate) | Reuse one collapse predicate in token summaries, greedy wrapping, leading trim, and Knuth–Plass terminal trimming | 14 | 14 | 0 |

| 27 (line geometry) | Share base indentation and available-width calculation across greedy, optimal, and float-aware composition | 49 | 54 | -5 |

| 2 (prepared direction) | Convert CSS direction during preparation and remove its special AST branch from cascade application | 16 | 13 | +3 |

| 2 (prepared miscellaneous properties) | Prepare hyphens, word-break, overflow-wrap/word-wrap, and box-sizing; delete misc family applier and routing arm | 47 | 53 | -6 |

| 2 (obsolete fallback routing) | Remove routes to unhandled family cases, duplicate whitespace conversion, and unused ApplyResult plumbing | 24 | 138 | -114 |

| 2 (prepared flex/grid fields) | Prepare remaining flex/grid scalar and enum properties; delete the layout family applier and routing arm | 23 | 33 | -10 |

| 2 (prepared text enums) | Prepare line-break, unicode-bidi, list-style-position, overflow-x/y, and text-overflow; remove their family branches and fallback routes | 58 | 51 | +7 |

| 2 (prepared decoration and stacking) | Prepare supported decoration-line flags and all z-index values; remove their AST fallback branches | 22 | 24 | -2 |

| 2 (prepared font family) | Serialize font-family lists during preparation; retain document-local interning and delete the typography fallback module | 18 | 38 | -20 |

| 2 (prepared font caps and list image) | Prepare native font-cap feature lists and trimmed list-image URLs; remove their fallback routes and URL helper | 27 | 37 | -10 |

| 2 (prepared shorthand caps) | Prepare font-variant caps alongside numeric/ligature features and share conversion with the native longhand | 44 | 44 | 0 |

| 2 (prepared border styles) | Convert physical/logical border styles during preparation; retain direction-dependent side selection and remove fallback routes | 24 | 33 | -9 |

| 2 (prepared colors) | Prepare fixed color conversion while preserving current-color dependencies; remove color-family fallback branches and duplicate helpers | 97 | 121 | -24 |

| 2 (prepared background presence) | Prepare background-image presence and remove early image scanning plus the none-value fallback | 6 | 7 | -1 |

| 2 (prepared text alignment) | Prepare physical/logical alignment intent, share contextual application, and delete the final text-family fallback module | 92 | 74 | +18 |

| 2 (prepared whitespace) | Prepare native white-space values and apply selected collapse/wrap targets through the native dispatcher | 38 | 27 | +11 |

| 2 (canonical side identity) | Carry the selection key into native application and resolve physical/logical sides using PropertyKey | 58 | 55 | +3 |

| 2 (keyed grid/typography application) | Replace source-property arguments with PropertyKey in prepared grid and typography application | 13 | 11 | +2 |

| 2 (native key dispatch) | Remove CSS Property from native application; prepare text-indent flags and dispatch by PropertyKey | 61 | 62 | -1 |

| 2 (prepared native keys) | Store native keys with values; apply the compact list and remove expanded-AST ranges and dispatch wrapper | 51 | 142 | -91 |

| 2 (prepared font resets) | Prepare keyed font feature resets; share their targets and remove special application branch | 32 | 32 | 0 |

| 2 (unified prepared application) | Store renderer values in the keyed value list; remove renderer pool and AST application input | 49 | 78 | -29 |

| 2 (white-space longhands) | Prepare collapse/wrapping separately and require a canonical key for every executable value | 89 | 95 | -6 |

| 2 (renderer shorthand longhands) | Expand text-wrap, font-variant, font-synthesis, and text-box before application; centralize selection | 53 | 66 | -13 |

| 2 (direct renderer preparation) | Emit keyed longhands while parsing; remove compound payloads and second expansion pass (production only) | 58 | 98 | -40 |

| 2 (single value enum) | Merge renderer payload/application into PreparedValue; unify feature storage and fix support-query consumer (production only) | 302 | 339 | -37 |

| 2 (shared spacing value) | Use one spacing payload/application for native and renderer letter/word spacing | 22 | 39 | -17 |

| 15 (sparse normalization) | Keep run ranges directly; remove dense character map, storage switch, and redundant source-index plumbing | 17 | 121 | -104 |

The first two entries retain the previously reported net counts; their full
addition/deletion breakdown was not retained here. Earlier patches still need
an end-to-end source audit against the current tree before closing the goal.

Finding 12 retains both sharing caches pending measurements, as the review
recommends. Hint collection now precedes cache lookup and is reused by cascade;
its performance impact has not been benchmarked. Custom-cascade admission and
the 256-entry limit are unchanged.

Finding 14 preserves the explicit formatting/text stages and transactional MathML
rollback. Their shared constructors build the tree and derived inputs; responsive
restore borrows the same state. Initial preparation now applies the supplied
publication paint policy consistently with reprepare. The ordinary layout API
uses the timed path and discards its timings. Session dispatch now follows the
selected stage directly, preserving the ownership, height-only, and paint paths.
Source audit completed; compilation and runtime verification remain outstanding.

Remaining findings: 1, 2, 5, 6, 9, 13, 15–24, 26–32. Findings 4, 5, 9, 13, 23, and 24 remain partial.

Finding 5's duplicate test-only path is removed. The broader reader overlay
remains coupled to findings 1, 13, and 20. Existing tests were adapted, not run.

Finding 13 removes per-declaration minimum enforcement. Consolidating the
remaining reader policy into the used-style overlay is still outstanding.

Finding 25 now stores sibling-sharing attribute ranges on PreparedSelector.
Both matchers now require TraversalContext, replacing SiblingIndex. Scope and
existing test callers supply it; context-free sibling and relative-selector
paths are removed. Ancestor candidate rejection and matching now share the exact
counted facts; per-frame bloom masks and their argument plumbing are removed.
The two SummaryTest constructors support the two engines intentionally retained
by finding 11. Its recommendation is one resolver entry point, now implemented,
not deletion of either engine without a measured need. No performance
measurements have been run.

Finding 10 now has one candidate/specificity loop and one cascade-target
execution path for events, custom properties, and property application. Element
cache admission, hints, and legacy justify-items inheritance remain element-only;
pseudo maps still rewind after application and disable substitution caching.
Custom-map cache insertion now happens during cascade rather than style storage.
Each pseudo target now matches and orders its rules once in store_pseudo_styles.
Its inheritance variants explicitly borrow that plan; CachedRuleMatches and its
last-target key are removed. Rule buffers are retained across targets/elements.
Finding 10 is implemented at source level. Syntax and whitespace checks passed;
compilation and runtime verification remain outstanding.

Finding 23 now uses native preparation failures in capability queries. Display,
position, visibility, and text-transform restrictions live in preparation;
declaration admission no longer repeats them. Constant aliases canonicalize once
before validation. Both static and substituted native declarations still pass
through prepare_declaration. Renderer-owned parsing, shorthand support, remaining
capability predicates, and failure reasons still need consolidation. Syntax and
whitespace checks passed; compilation and runtime verification remain outstanding.

Finding 23 renderer support follow-up: tokens_are_supported is removed. Capability
queries use PreparedRendererValue::parse for renderer-owned values, preserving
native-alias routing and the public syntax-error distinction for object-position
and outline-offset. Generated-content-specific reasons precede general parse
failures. Widows/orphans preparation now consumes integer tokens directly, with
positive-value validation and the existing u8 clamp. The existing allocation
test calls preparation directly; it was not run. Syntax and whitespace checks
passed. The broader unified preparation result/failure reasons remain pending.

Finding 23 shorthand/grid follow-up: capability queries now expand native
shorthands and prepare their longhands using the cascade’s existing functions.
Duplicate list-style, outline, decoration-style and grid support predicates are
removed. Decoration-line flags, reserved grid placement names, and zero named
line indices now fail during preparation; existing numeric bounds and automatic
track-repeat validation remain there. Syntax validation remains separate.
Syntax and whitespace checks passed; no compilation or tests were run. The
unified preparation result and failure reasons are still outstanding.

Finding 23 admission follow-up: PreparedRendererValue::prepare now validates AST
admission, defers CSS-wide/substituted values, and returns prepared raw values.
Static and substituted declarations consume that result; renderer capability
queries use the same entry. Native fallback conversion still runs in native
preparation. The existing eligibility test now targets preparation, without
being run. Syntax and whitespace checks passed; compilation remains unrun.
Containment is a remaining semantic distinction: layout intentionally applies
the size part of strict containment (covered by an existing source test), while
capability queries reject incomplete containment support. That behavior has not
been changed. Unified failure reasons and remaining syntax/query distinctions
are still outstanding.

Finding 23 query follow-up: public queries and @supports share
parsed_declaration_support, including AST validation and capability evaluation.
Both callers canonicalize aliases before parsing, so recursive alias reparsing
is removed. Native fallbacks already validated by preparation are not parsed
again for syntax. Prepared containment retains its existing fully_supported
flag; queries read it without reparsing while layout still applies size only.
Syntax and whitespace checks passed; no compilation or tests were run.
Unified failure reasons and the remaining raw syntax distinctions remain open.

Finding 23 failure-reason follow-up: renderer preparation now reports
InvalidSyntax or an UnsupportedStyleFeature. prepare_support calls renderer
preparation, expands native shorthands, and maps native conversion failures to
unsupported values. Both public queries and @supports consume that result.
Generated-content diagnostics now originate in failed preparation; object-position
and outline-offset grammar checks run only after preparation fails, to distinguish
invalid syntax from an unsupported value. The separate raw syntax dispatcher is
removed. Containment and background feature policies remain explicit query
restrictions because layout currently supports parts of those declarations.
Syntax and whitespace checks passed; compilation and runtime verification remain
outstanding. Finding 23 remains partial pending reconciliation of these partial
support policies and the combined declaration representation in finding 9.

Finding 9 storage follow-up: each declaration now owns one metadata record
containing target/flag data, expansion range, renderer index, and native values.
The block retains shared targets, longhands, and renderer values, with no new
per-declaration allocation. Append no longer passes seven mutable vectors to a
free function. Rule readers use block accessors. Existing metadata assertions
were adapted; syntax and whitespace checks passed, without compilation or tests.
The AST, value pools, repeated event views, and separate substitution record
remain to be consolidated; this is an intermediate storage change.

Finding 9 event-access follow-up: rules and inline styles expose their prepared
block and declaration offset once. DeclarationEvent builds its view through
that common block interface; metadata access no longer repeats rule-source
lookup for every field. Selection and application reuse the view, including
viewport dependency during admission. Rule IDs for substitution caching no
longer fetch the property again. Removed accessors have no remaining callers.
Syntax and whitespace checks passed; compilation and runtime checks remain
unrun. Events still hold indices, and original/expanded/substituted values
still use separate representations, so finding 9 remains partial.

Findings 9/23 shared-preparation follow-up: PreparedValue::prepare now owns
renderer admission, shorthand expansion, and native conversion for stylesheet
blocks, inline styles, hints, substituted values, and capability queries. Its
viewport-dependency calculation is reused by static and substituted storage.
Static failures truncate appended longhands and remain ineligible; substitution
failures still become unset through the existing resolver. The redundant
expansion_prepared argument and apply-time expansion of unchanged declarations
are removed. Shared pools and retained substitution storage remain in place.
Syntax and whitespace checks passed; no compilation or tests were run. The
original AST and separate resolved storage remain to be unified for finding 9;
partial feature support still needs reconciliation for finding 23.

Finding 9 application follow-up: the cascade builds one effective declaration
view containing the selected targets and the applicable property, longhands,
renderer value, and native values. Property dispatch consumes that view without
knowing ResolvedDeclaration or choosing storage sources. The substitution-state
bit is removed: at its only read, owned values have just come from substitution
and alias canonicalization has not run yet. Reset still clears ownership before
each resolution. Syntax and whitespace checks passed; no compilation or tests
were run. Persistent static and substituted storage remain separate.

Finding 28 sparse-storage follow-up: first-letter selection emits ranges directly
and sorts them into normalized character order, independent of item-arena order.
Case conversion remaps endpoints through its existing boundary map; empty ranges
are removed and adjacent equal-style ranges merge before shaping. This removes
both full-length first-letter override buffers and the final dense compaction
scan. Production changes are +61/-58 (+3); existing test adaptations are
+15/-66 (-51), for a combined +76/-124 (-48). Counts use the pre-patch snapshot,
not HEAD, and exclude this documentation. Syntax parsing with rustfmt and patch
whitespace checks passed; no compilation or tests were run. Finding 28 remains
partial: collapse and case conversion still rebuild buffers separately, and
transform/language metadata still occupies full-length temporary arrays.

Finding 28 case-metadata follow-up: transform and language data now live in one
CaseRun per effective style range instead of two character-length arrays. The
main rewrite advances through sorted ranges once, preserving default behavior
for gaps. Greek uppercase and Dutch capitalization query the same ranges;
source offsets, wrapping ownership, final-sigma context, and glyph-boundary
remapping remain unchanged. The no-transform fast path still avoids allocating
metadata. This trades +30 production lines (+57/-27) for storage proportional
to style ranges; highly fragmented text is not guaranteed to use less memory.
Counts exclude documentation and earlier patches. Rustfmt syntax parsing and
patch whitespace checks passed; compilation, tests, and benchmarks were not
run. The shared collapse/case-conversion rewrite remains outstanding.

Findings 19/26 sizing-policy follow-up: one LayoutReader match describes table,
row, cell, and ordinary sizing rules. ResolvedBoxSizing carries those rules into
final border-size resolution, which no longer reads the engine or topology.
Intrinsic measurement reuses the minimum-width and cell-margin decisions.
Authored cell widths still contribute to intrinsic measurement while assigned
cell layout ignores them; this distinction is preserved. Collapsed borders,
empty-table final-size ownership, and formatting-context dispatch are unchanged
and remain outstanding parts of these findings. Production delta: +45/-23
(+22), excluding this documentation. Rustfmt syntax parsing and patch whitespace
checks passed; no compilation or tests were run.

Finding 19 decoration-ownership follow-up: table placement now includes the
resolved grid insets when publishing its caption-separated decoration rectangle.
The insets come directly from ResolvedBoxSizing through TableLayoutInput, so the
four resulting coordinates retain the former content-rectangle-plus-insets
calculation. Generic box finishing no longer identifies tables, fetches that
rectangle, or rewrites it. Table placement is the only layout writer of this
rectangle; captionless and empty-grid conditions are unchanged. Production
delta: +9/-21 (-12), excluding documentation. Rustfmt syntax parsing and patch
whitespace checks passed; no compilation or tests were run. Other sizing,
border-ownership, and formatting-context work remains open.

Finding 19 edge-policy follow-up: SizingPolicy now determines whether the context
supplies its own padding/border edges. Its box-model adjustment replaces three
separate table-sizing paths; generic sizing no longer checks table mode/display
or border-collapse. Request-supplied cell borders still apply after adjustment.
The existing final-sizing fallback for retained Table/InlineTable display now
also applies during intrinsic measurement, making their edge models consistent.
This is a behavior change for any such box whose mode is not Table; ordinary
cells/rows and noncollapsed tables retain their prior edge model. Production
delta: +33/-32 (+1), excluding documentation. Rustfmt syntax parsing and patch
whitespace checks passed; no compilation or tests were run. Final-size ownership,
table auto-width resolution, and post-layout block alignment remain to be audited
before finding 19 can be closed.

Finding 17 trailing-margin follow-up: finish_flow and used_after_margin now take
the existing ResolvedBoxSizing. Trailing-margin collapse reads its min/max
height constraints and resolved bottom margin, and reuses the flow's existing
parent-edge context. It no longer resolves authored percentages against the
child percentage-height basis or repeats padding/border and reader-margin work.
Content-dependent constraints use the completed natural height, as final sizing
does; the clearance branch still retains only the parent's own bottom margin.
The unused BlockFlowInput.parent_cb_width field is removed. Production delta:
+22/-47 (-25), excluding documentation. Rustfmt syntax parsing and patch
whitespace checks passed; no compilation or tests were run. Repeated sizing
between margin analysis, float-fit probes, and actual layout remains open.

Finding 17 sizing-cache follow-up: resolve_box_sizing reuses a stored result when
its full request compares equal, including containing dimensions, float width
limit, assigned dimensions/definiteness, purpose, used cell borders, measurement
mode, and first-line indent. Each box retains only its latest request. Results
reset at final-pass entry and exchange with isolated measurement scratch along
with the existing intrinsic/flex-grid caches. Sizing reads immutable pass inputs;
its intrinsic text/child measurement does not depend on published geometry or
the active external float bands. NaN requests miss the equality check. This
implements the requested per-pass cache alongside the earlier trailing-margin
reuse; source changes for finding 17 total -14 lines across both patches.
This patch adds 15 and removes 4 production lines (+11), excluding documentation.
Rustfmt syntax parsing and whitespace checks passed; no compilation, tests, or
benchmarks were run. The additional per-box storage and actual cache-hit benefit
remain unmeasured, and runtime verification is still outstanding.

Finding 18 numeric-limit follow-up: ResolvedHorizontalSizing carries f64 content
limits instead of PreferredSize bounds and no longer carries their conversion
inset. Width constraints resolve those limits once; internal ratio/auto-width
calculation and table/flex-grid consumers clamp against the same pair. The
minimum still wins conflicting bounds, and the existing nonnegative clamp is
preserved. Removed resolve_content_width has no remaining callers. Authored
bounds remain available to the replaced-element solver. Production delta:
+41/-56 (-15), excluding documentation. Rustfmt syntax parsing and patch
whitespace checks passed; no compilation or tests were run. Finding 18 remains
partial: intrinsic/stretch widths still pass through PreferredSize::Px inside
constraint preparation, and preferred width and other conversion paths remain.

Finding 18 intrinsic-width follow-up: ordinary box constraint preparation now
resolves min-content, max-content, fit-content, and locally owned stretch into
f64 content values. Numeric bounds and the preferred-content override consume
those values directly, without adding the border-box inset, casting to f32,
and subtracting the inset again. Authored preferred values remain available for
auto detection; the replaced solver retains its existing authored-axis path.
Flex/grid-owned stretch keeps its prior handling. Production delta: +48/-42
(+6), excluding documentation; the two finding-18 patches total -9 lines.
Rustfmt syntax parsing and patch whitespace checks passed; no compilation or
tests were run. Replaced stretch, smart image sizing, and the broader preferred
size/box-model conversions still need consolidation before closing finding 18.

Finding 18 resolved-width follow-up: ResolvedHorizontalSizing now stores numeric
content/min/max widths and an auto_width fact instead of retaining PreferredSize.
All its former preferred-width consumers distinguished Auto from every other
variant; they now read that fact directly. The fact is computed after smart
image preference handling, preserving the previous branch choices. Three
consumer modules no longer import PreferredSize. Production delta: +20/-32
(-12), excluding documentation; finding 18 now totals -21 lines across its
three patches. Rustfmt syntax parsing and patch whitespace checks passed; no
compilation or tests were run. Internal authored constraints and replaced-image
conversions remain outstanding.

Finding 18 reader-image follow-up: smart_standalone_image_width now returns an
f64 content width. Block sizing and inline image preparation pass it directly
to resolve_replaced_content_size through its optional numeric-width argument.
The solver uses that definite content width for ratio transfer and constraints;
other callers pass None. The old content-to-border-box conversion and f32 cast
are removed. Auto-width classification accounts for the numeric override, and
smart-image margin/min/max suppression remains in place. Existing test calls
were adapted, not run or newly created. Production delta: +21/-18 (+3); test
adaptations: +20/-8 (+12); combined +41/-26 (+15), excluding documentation.
Finding 18's cumulative production delta is -18 lines (-6 including adapted
test lines). Rustfmt syntax parsing and patch whitespace checks passed; no
compilation or tests were run. Replaced stretch still uses an authored-value
conversion, and finding 18 remains partial.

Finding 18 replaced-stretch follow-up: the replaced solver now accepts numeric
preferred/minimum/maximum content widths. Its bound resolver selects numeric
overrides before applying the minimum-wins rule; untouched axes and flex auto-min
queries use the same resolver without overrides. Constraint preparation no
longer creates PreferredSize::Px for stretch, including replaced elements.
Smart image sizing clears numeric min/max overrides alongside authored limits;
flex/grid-owned stretch retains its existing path. Production delta: +56/-49
(+7); existing test-call adaptations: +8/-8 (0); combined +64/-57 (+7), excluding
documentation. Finding 18's cumulative production delta is -11 lines (+1 including
existing test adaptations). Rustfmt syntax parsing and whitespace checks passed;
no compilation or tests were run. The intrinsic/stretch/image width round trips
identified in box sizing are removed; broader conversion and margin-distribution
consolidation still remains before closing finding 18.

Finding 18 automatic-margin follow-up: resolve_auto_start_margin now owns the
free-space calculation and auto-margin distribution used by the block/table
margin resolver, block inline alignment, and absolute positioning. Overflow
policy remains explicit: horizontal absolute/table distribution selects an end,
vertical absolute positioning splits negative space, and block alignment keeps
its existing margin when space is not positive. The no-auto RTL overconstraint
rule and absolute static/inset placement stay with their callers. Production
delta: +60/-60 (0), excluding documentation; existing tests are unchanged.
Rustfmt syntax parsing and whitespace checks passed; no compilation or tests
were run. Finding 18's production total remains -11 lines (+1 including earlier
test-call adaptations). Remaining box-model conversions still need auditing.

Finding 18 axis-conversion follow-up: AbsoluteAxis::stretched_border_size uses
the common content-size resolver and adds padding/border once to publish a
border size. It preserves its specified-box stretch basis and minimum-wins
clamp. Unresolved content bounds now keep the fallback directly instead of
adding padding/border to that fallback again. The fixed-basis flex-column path
uses the same resolver under its existing Px-only eligibility guards and
resolves the minimum once. Production delta: +28/-25 (+3), excluding documentation.
Finding 18's cumulative production delta is -8 lines (+4 including existing
test-call adaptations). Rustfmt syntax parsing and patch whitespace checks
passed; no compilation or tests were run.

Finding 18 source coverage: ResolvedHorizontalSizing contains numeric content
limits plus an auto-width fact; resolve_width_constraints no longer constructs
PreferredSize::Px; reader image and replaced stretch widths remain numeric;
trailing margins reuse resolved vertical sizing; both named remaining conversion
sites use the shared resolver; block/table/absolute auto-margin paths delegate
to one distribution helper. These cover the finding's named source changes.
Compilation and runtime evidence remain outstanding, so this is not a verified
end-to-end completion claim for the overall goal.

Finding 21 paint-ancestry follow-up: the tree already cached final owner/item
keys, but each new key still walked its ancestors. Missing group paths now
build parent-first using an explicit pending stack and stop at a cached path.
The key includes group/table collection flags because positioned descendants
escape temporary groups and backgrounds collect collapsed-table groups. Boxes
that contribute no group share the inherited Arc allocation. Direct-owner
background rank is stored separately from the path; retained split-inline
ancestors remain in the local group sequence, reversed before appending to the
parent prefix. Nearest DOM source lookup also memoizes every traversed ancestor,
including a distinct no-DOM result so anonymous fallback remains owner-local.
All added caches clear with the existing paint cache, and table-local caches
prepare only requested ancestry instead of scanning the entire document.
Production delta: +71/-26 (+45), excluding documentation. Rustfmt syntax parsing
and whitespace checks passed; no compilation, tests, or benchmarks were run.
The added cache memory and speed benefit remain unmeasured. Finding 21's broader
finalization-pass consolidation and complete stacking-fact storage remain open.

Finding 21 simplification follow-up: path reconstruction now carries its own
background-ownership flag with the cached path. An immediate cache hit already
has the correct flag; otherwise the final rebuilt entry supplies the requested
box's flag. Local ancestor classification only distinguishes a real owner from
retained virtual inline ancestry, so its duplicate box-index argument and two
target checks are removed. Production delta: +6/-14 (-8), excluding documentation;
the paint-path patches now total +37 lines. Rustfmt syntax parsing and whitespace
checks passed; no compilation, tests, or benchmarks were run. Other finding-21
work and runtime verification remain open.

Finding 31 absolute-queue follow-up: pending absolute boxes now live in FIFO
queues keyed by containing block. take_for looks up the target queue and pops
its front; empty queues are removed. The containing-block field is no longer
repeated in each entry. Layout still resolves target geometry lazily once and
rechecks the queue after each box, so newly deferred work remains ordered.
This removes document-wide scans and middle removal while preserving per-target
source order. It trades the single deque allocation for per-target queue storage;
that allocation/performance tradeoff has not been benchmarked. The existing
source-order/allocation test was adapted, not run or newly created. Production
delta: +10/-7 (+3); test adaptation: +8/-9 (-1); combined +18/-16 (+2), excluding
documentation. Rustfmt syntax parsing and whitespace checks passed; no compilation
or tests were run. Finding 31's other observations remain open.

Finding 31 cell-dependency follow-up: cell_height_dependency memoizes its existing
structural/style scan by cell index. The cache clears at final-pass entry and
exchanges with isolated measurement scratch, so repeated intrinsic/final table
layout reuses the same dependency result. The predicate, traversal boundaries,
and strongest-dependency early exit are unchanged; dimensions and geometry are
not inputs to this classification. Only queried cells occupy the map. This
stores a per-pass derived fact rather than changing the permanent box format.
Production delta: +26/-6 (+20), excluding documentation. Rustfmt syntax parsing
and whitespace checks passed; no compilation, tests, or benchmarks were run.
The storage/performance tradeoff remains unmeasured, and finding 31's other
observations remain open.

Finding 32 selector-validity follow-up: fragment compilation now validates each
style rule once and stores the boolean in PreparedRuleDeclarations. Linking
reads that result before recording media dependencies or emitting rules, retaining
the existing skip behavior and rule indices. Original selectors remain intact
for stylesheet reconstruction. RuleWalk no longer owns a validation cache or
its fragment lifetime. This adds one boolean per rule (including possible struct
padding) while removing repeated validation across media environments.
Production delta: +13/-14 (-1), excluding documentation. Rustfmt syntax parsing
and whitespace checks passed; compilation and tests were not run. Supports
conditions, scope-selector validation, and the public style-entry-point overlap
remain open under finding 32.

Finding 32 static-condition follow-up: the existing fragment traversal evaluates
supports conditions and scope-selector validity, storing their booleans in
preorder. Linking consumes those results for every supports/scope node, including
skipped and media-inactive branches, so nested traversal remains aligned. The
existing active/dependency/skip decisions are retained; original AST conditions
and scope selectors remain available for reconstruction. All nested rule kinds
share one recursive call. The extra storage and iterator account for the net
increase; no additional traversal or lookup map was introduced.
Production delta: +33/-19 (+14), excluding documentation; cumulative finding 32
production delta is +13. Rustfmt syntax parsing and whitespace checks passed;
compilation and tests were not run. The public style-entry-point overlap in
finding 32 remains open.

Finding 32 entry-point follow-up: source inspection established that both public
paths already share resolve_document_with_program. StyleRequest and
style_document now live beside that resolver in program.rs; the separate request
module and program::apply forwarding function are removed. StyleRequest::apply
calls the canonical borrowed resolver directly and wraps the result in the owned
StyledDocument. Input copying uses the existing Clone implementation through
to_vec. Public exports, owned/borrowed semantics, timings, scope bindings, reader
overrides, and font-face ownership are preserved. This removes an internal layer;
it does not remove the two public usage forms described in finding 32.
Production delta: +104/-129 (-25), including relocated code and deleting
request.rs, excluding documentation. Cumulative finding 32 production delta is
-12. Rustfmt syntax parsing, whitespace checks, and a search for obsolete module
references passed. Compilation and tests were not run; broader public API
consolidation remains open.

Finding 31 inline-policy follow-up: PreparedInlinePlanKey and
ParagraphDependencies now retain TextCompositionPolicy directly. Both previously
listed book-optimization and hyphenation-quality booleans separately. The three
current enum variants correspond exactly to their three reachable boolean pairs,
so cache partitioning is unchanged; Hash is derived on the policy for token keys.
Existing fixtures now select the corresponding enum variant. Production delta:
+7/-10 (-3); existing test adaptations: +7/-4 (+3); combined +14/-14 (0), excluding
documentation. Rustfmt syntax parsing and whitespace checks passed; no compilation
or tests were run. Other dependency fields and cache-wide generation boundaries
remain separate and still need review. FlowParticipation already derives from
stored BoxFacts rather than style lookups; no redundant fact was added here.

Finding 31 font-metric follow-up: layout no longer recognizes Ahem by its family
string. Text preparation and both inline/container struts use the existing used
style normal-line-height resolution, which already receives the selected face's
normal_line_height_ratio from ShapedFontMetrics. Preserved tabs use the selected
face's space/ch advance ratios. The existing taller-fallback-glyph expansion
remains intact. The now-unconditional text height no longer uses an Option.
Production delta: +9/-43 (-34), excluding documentation. Rustfmt syntax parsing,
whitespace checks, and a search for removed helper references passed. No
compilation or tests were run. This deliberately changes backends that declare
Ahem but omit measured font metrics: they now get the same metric fallbacks as
other fonts. Synthetic Ahem fixtures that relied on the removed override remain
unverified and may require metric-provider updates; no test code was added.
Other finding 31 observations remain open.

Finding 31 standalone-row audit: html-format's build_layout_inputs_from calls
LayoutTreeBuilder::build_from, which builds the selected element with no parent.
The root adjustment in build_classified_element only converts display:contents;
a selected table-row therefore remains a TableRow root. Generic box content
layout dispatches it to layout_table_row. Ordinary parent fixup does not prove
this fallback dead. No production change (0 lines); the equal-width fallback is
retained pending a decision about scoped table-internal roots.

Finding 27 leading-boundary follow-up: the existing greedy leading-token helper
now belongs to the wrapping module and is also used by KpSegmentPlan::build.
This removes its separate whitespace loop and makes leading authored zero-width
breaks skip consistently in both algorithms. Preserved spaces and hard-break
boundaries keep the existing helper behavior; no width-dependent work or plan
allocation is added to the greedy path. Production delta: +26/-34 (-8), excluding
documentation. Rustfmt syntax parsing and whitespace checks passed; no compilation
or tests were run. Shared breakpoint selection, trailing-space policy, and
hanging-width decisions remain open under finding 27.

Finding 27 edge-space follow-up: token_is_collapsible_line_edge_space now accepts
TokenMetadata and the existing MetricsCursor. Its short-circuit predicate is
unchanged: only soft tokens whose whitespace policy does not preserve spaces
collapse at line edges. Summary construction, greedy fitting/content tracking,
shared leading trimming, and Knuth–Plass terminal-suffix trimming use that one
predicate. Metric access remains conditional on the soft-token classification;
cursors preserve uniform/indexed metric lookup without allocating storage.
Production delta: +14/-14 (0), excluding documentation; cumulative finding 27
production delta is -8. Rustfmt syntax parsing, whitespace checks, and caller
signature inspection passed. Compilation and tests were not run. Full shared
breakpoint selection and hanging-width policy remain open.

Finding 27 line-geometry follow-up: the former greedy-only line_constraints
function is now LineConstraints::new. Optimal composition uses its indent and
width for empty forced lines, first/later segment candidates, and selected lines;
float-aware composition uses the same base constraints and adjusts only its
paragraph-relative tab origin. Existing first-line/each-line decisions,
hanging-punctuation additions, and float-origin arithmetic are preserved.
Production delta: +49/-54 (-5), excluding documentation; cumulative finding 27
production delta is -13. Rustfmt syntax parsing, whitespace checks, and caller
inspection passed; compilation and tests were not run. Shared breakpoint
selection and remaining punctuation-specific width adjustments are still open.

Finding 2 prepared-direction follow-up: PreparedNativeValue stores the engine's
TextDirection, converted by the existing common preparation path. Applying it
writes the prepared enum and retains logical text-alignment rebinding. The
central apply_longhand dispatcher no longer recognizes or converts the CSS
Direction AST. CSS-wide keywords still leave through the existing preparation
guard, and static/substituted native values use the same conversion.
Production delta: +16/-13 (+3), excluding documentation. The new prepared enum
variant accounts for the small net growth while removing repeated conversion.
Rustfmt syntax parsing, whitespace checks, and source-path inspection passed;
compilation and tests were not run. The remaining raw-property routing and
property identity consolidation in finding 2 are still open.

Finding 2 miscellaneous-family follow-up: preparation now converts hyphenation,
word-break, overflow-wrap (including word-wrap alias), and box-sizing values to
engine enums. Prepared application writes the fields directly. The former
properties/misc.rs applier, its ApplyResult plumbing, and its central routing
arm are deleted. Existing enum mappings and vendor-prefixed property patterns
are preserved. These declarations now occupy native prepared-value slots rather
than converting their AST on every application; that preparation/storage tradeoff
has not been benchmarked. Production delta: +47/-53 (-6), excluding documentation;
cumulative finding 2 production delta is -3. Rustfmt syntax parsing, whitespace
checks, and source inspection for removed routing references passed. Compilation
and tests were not run. Remaining property families and property identity
consolidation are still open.

Finding 2 fallback-routing follow-up: the central fallback match now lists only
properties actually handled by each remaining family applier. Removed cases
previously reached an Unhandled result with no property mutation; prepared native
and renderer dispatch still runs before this fallback. Unparsed z-index/flex-basis
fallback calls likewise had no matching family implementation. WhiteSpace is
already applied with selected-target filtering in compute_declaration, so its
unreachable whole-value family branch is removed. ApplyResult had no inspected
callers; family functions now return unit and unmatched properties remain no-ops.
Production delta: +24/-138 (-114), excluding documentation; cumulative finding 2
production delta is -117. Rustfmt syntax parsing, whitespace checks, and source
inspection of family match coverage and ApplyResult references passed. Compilation
and tests were not run. Remaining AST-backed families and property identity
consolidation are still open.

Finding 2 flex/grid-field follow-up: flex-direction, flex-wrap, flex-grow,
flex-shrink, order, justify-content, and grid-auto-flow now store their converted
engine values in PreparedNativeValue. Existing conversion helpers, flex-factor
clamping, and vendor-prefix patterns are unchanged. Application assigns fields
without inspecting the source AST for these values. The former properties/layout.rs
family applier and its central routing arm are deleted. These properties now
occupy prepared native slots; storage/runtime effects remain unbenchmarked.
Production delta: +23/-33 (-10), excluding documentation; cumulative finding 2
production delta is -127. Rustfmt syntax parsing, whitespace checks, and source
inspection of imports, field types, and removed module references passed.
Compilation and tests were not run. Remaining AST-backed families and property
identity consolidation are still open.

Finding 2 text-enum follow-up: six context-independent text/box properties now
store engine enum values during preparation. Their original mappings, including
all line-break and bidi variants and prefixed text-overflow patterns, are retained.
Application assigns the prepared values directly. Parent/direction-dependent
alignment, string interning, and color resolution remain in the text family.
Production delta: +58/-51 (+7), excluding documentation; cumulative finding 2
production delta is -120. The new prepared variants account for the net growth
and replace per-application AST conversion with retained values. Rustfmt syntax
parsing, whitespace checks, and source comparison of mappings passed. Compilation,
tests, and benchmarks were not run. Remaining AST-backed families and property
identity consolidation are still open.

Finding 2 decoration/stacking follow-up: prepared z-index now stores Option<i32>
for both auto and integer values; the existing raw integer fallback enters the
same representation with Some. Decoration-line preparation retains its unsupported
flag guard and stores the converted engine flags when admitted. Application
assigns these prepared values directly, removing the corresponding text and
box-model fallback branches and central routes. Production delta: +22/-24 (-2),
excluding documentation; cumulative finding 2 production delta is -122.
Rustfmt syntax parsing, whitespace checks, and source inspection of enum mappings
and Copy field types passed. Compilation and tests were not run. Remaining
AST-backed families and property identity consolidation are still open.

Finding 2 font-family follow-up: PreparedTypography stores the serialized family
list in a boxed string. Preparation uses the same per-family ToCss writer and
comma-space separator as the former intern_font_family helper. Application only
interns that prepared string into the destination style store, preserving
store-local IDs. This removes repeated family serialization, the old helper,
and the properties/typography.rs applier and route. Prepared strings now retain
additional declaration storage; runtime and allocation tradeoffs are unmeasured.
Production delta: +18/-38 (-20), excluding documentation; cumulative finding 2
production delta is -142. Rustfmt syntax parsing, whitespace checks, and source
inspection of the preparation path and removed references passed. Compilation
and tests were not run. Remaining AST-backed families and property identity
consolidation are still open.

Finding 2 font/list follow-up: native font-variant-caps now retains the feature
list produced by the existing conversion helper, and application uses clone_from
to reuse destination vector capacity where possible. List-style-image retains
its trimmed URL or None; the destination style store still owns string interning.
Empty URLs and non-URL image variants preserve their former None behavior. The
old text-family branches, central routes, and URL interning helper are removed.
Production delta: +27/-37 (-10), excluding documentation; cumulative finding 2
production delta is -152. Rustfmt syntax parsing, whitespace checks, and source
inspection passed; compilation and tests were not run. The renderer-owned
font-variant shorthand still converts its caps component during application;
that path, other AST-backed families, and property identity consolidation remain
open. Prepared storage and allocation effects have not been benchmarked.

Finding 2 shorthand-caps follow-up: PreparedRendererValue::FontVariant now retains
three engine feature lists. The caps mapping moved from cascade values to the
syntax font-feature module and is shared by shorthand and native-longhand
preparation. Shorthand application copies the selected prepared caps list with
clone_from, matching its numeric/ligature handling and preserving target selection.
No caps AST conversion remains in property application. Production delta:
+44/-44 (0), excluding documentation; cumulative finding 2 production delta
remains -152. Rustfmt syntax parsing, whitespace checks, and source inspection
of both callers passed; compilation and tests were not run. Remaining AST-backed
families and property identity consolidation are still open.

Finding 2 border-style follow-up: the eight physical/logical border-style
longhands now share a prepared BorderStyle value. The existing conversion helper
runs during preparation; application resolves the physical side using the current
direction and calls the existing field setter. The old box-model conversion
branch and central fallback routes are removed. Repeated qualified physical_side
calls now use an import. Production delta: +24/-33 (-9), excluding documentation;
cumulative finding 2 production delta is -161. Rustfmt syntax parsing, whitespace
checks, and source inspection passed; compilation and tests were not run.
Direction-dependent property-side routing, remaining AST-backed families, and
property identity consolidation are still open.

Finding 2 color follow-up: PreparedColor retains a fixed RGBA value or an
unresolved current-color value, separately from whether the authored value was
direct currentColor. Preparation preserves the fixed light-scheme selection,
system-color mappings, RGB conversion, and black fallback. Color application
uses the parent's color for the color property and the element's color elsewhere.
Direct currentColor keeps background/border flags and decoration references;
currentColor selected through light-dark retains its previous resolved-value
behavior without acquiring those flags. Border sides still resolve at application.
The old paint applier, box-model color applier, decoration branches, and duplicate
color setters are removed; presentational hints retain their conversion entry.
Production delta: +97/-121 (-24), excluding documentation; cumulative finding 2
production delta is -185. Rustfmt syntax parsing, whitespace checks, and source
comparison of color dependencies and remaining callers passed. Compilation and
tests were not run. Text alignment, background-image presence, and property
identity consolidation remain open; prepared storage effects are unbenchmarked.

Finding 2 background-presence follow-up: native background-image declarations
now retain the existing property_uses_background_image result as a prepared
boolean. Background shorthands expand to that longhand before native preparation.
Application directly sets presence for both image and none values, eliminating
the early AST scan and separate none fallback. Unsupported-image presence policy
and declaration eligibility are unchanged. Production delta: +6/-7 (-1),
excluding documentation; cumulative finding 2 production delta is -186.
Rustfmt syntax parsing, whitespace checks, and source inspection of shorthand
expansion passed. Compilation and tests were not run. Text alignment and property
identity consolidation remain open.

Finding 2 text-alignment follow-up: PreparedTextAlign represents physical values,
start/end, match-parent, auto, and the existing unspecified last-line fallback.
TextAlign and TextAlignLast share contextual resolution and update their distinct
fields before the existing logical-alignment rebinding. Root match-parent still
acts as start; non-root match-parent still freezes the parent's physical result.
Justify-all and last-line explicitness preserve their prior behavior. The text
family module and the last raw-property routing match are removed. The extra
prepared intent representation accounts for the net increase; selection and
special shorthand handling still retain AST dependencies elsewhere.
Production delta: +92/-74 (+18), excluding documentation; cumulative finding 2
production delta is -168. Rustfmt syntax parsing, whitespace checks, and source
comparison of alignment branches passed. Compilation and tests were not run.
Property identity consolidation and removal of the remaining AST dependencies
are still open.

Finding 2 whitespace follow-up: native white-space values now enter the prepared
native enum. Native application receives the existing selected-target predicate
and independently applies WhiteSpaceCollapse and TextWrapMode using the original
with_wrapping operations. The declaration loop retains only the target-selection
exception for this compound property; its CSS enum conversion and application
branch are removed. This keeps phase and partial-longhand selection intact while
moving conversion to preparation. Production delta: +38/-27 (+11), excluding
documentation; cumulative finding 2 production delta is -157. The prepared variant
and selected-target plumbing account for the net growth. Rustfmt syntax parsing,
whitespace checks, and caller inspection passed. Compilation and tests were not
run. Shorthand reset metadata and property identity consolidation remain open.

Finding 2 side-identity follow-up: compute_declaration retains its PropertyKey
lookup and passes it through native application. Physical/logical side selection
now matches that engine key, removing its parser PropertyId dependency. Every
side case was checked against the property catalogue; side indices and direction
mapping are unchanged. The key remains optional for compound/raw declarations
without a single native catalogue identity; the side resolver requires one, and
none of those compound declarations enter it. Production delta: +58/-55 (+3),
excluding documentation; cumulative finding 2 production delta is -154.
Rustfmt syntax parsing, whitespace checks, catalogue coverage, and caller
inspection passed. Compilation and tests were not run. Other application-time
AST matches and preparation-time key ownership remain open.

Finding 2 grid/typography identity follow-up: prepared grid and typography apply
functions now accept PropertyKey instead of the CSS Property AST. Row/column,
grid-line, and letter/word-spacing destination checks use that key; all value
resolution and document-local interning remain unchanged. Every property accepted
by these two preparers was checked against the catalogue before requiring its
key at dispatch. Production delta: +13/-11 (+2), excluding documentation;
cumulative finding 2 production delta is -152. Rustfmt syntax parsing, whitespace
checks, and source-level catalogue coverage passed. Compilation and tests were
not run. The outer native dispatcher and shorthand reset handling still retain
AST dependencies and remain open.

Finding 2 native identity follow-up: PreparedNativeValue::apply no longer receives
or inspects the lightningcss Property AST. Size, length, color, inset, radius,
and alignment destinations use PropertyKey. Text-indent preparation now retains
its hanging/each-line flags alongside its length, including viewport-height
tracking. Raw flex-basis fallback uses its canonical FlexBasis key. Grouped key
patterns keep this migration at +61/-62 (-1) production lines, excluding
documentation; cumulative finding 2 production delta is -153. Rustfmt syntax
parsing, changed-line whitespace checks, catalogue coverage, and caller inspection
passed. Compilation and tests were not run. The outer declaration loop still
uses AST identity for selection, shorthand resets, and renderer dispatch;
unifying those paths and moving key ownership into preparation remain open.

Finding 2 prepared native-key follow-up: preparation now stores each native value
with its PropertyKey instead of retaining index-aligned optional value slots.
Application walks this compact list in preparation order; it no longer walks
expanded AST properties or repeats property-name lookup. White-space retains
its selected-component handling, and font-size/line-height resolution bases use
the stored key. The redundant apply_longhand wrapper, PreparedExpansion ranges,
expanded-state accessors, and expanded-AST fields on DeclarationView are removed.
Stylesheet expansion storage is now reusable preparation scratch and is cleared
after each declaration. Substituted declarations use the same keyed preparation.
Production delta: +51/-142 (-91), excluding documentation; cumulative finding 2
production delta is -244. Rustfmt syntax parsing, changed-line whitespace checks,
caller/type-use inspection, and source review of expansion order and selection
passed. Existing test bodies were unchanged; compilation and tests were not run.
Renderer dispatch and font shorthand resets still inspect the source AST, so
finding 2 remains partial.

Finding 2 font-reset follow-up: font shorthand preparation emits empty feature
values for kerning, feature settings, numeric variants, and ligatures before its
expanded longhands. These values use the ordinary selected-key/phase application
path. The existing caps feature value is generalized to FontFeatures and reused
for the resets; cloning an empty feature vector clears its destination. A shared
FONT_RESET_TARGETS list keeps target selection and preparation aligned. The
application-time Property::Font reset branch is removed. Production delta:
+32/-32 (0), excluding documentation; cumulative finding 2 production delta
remains -244. Rustfmt syntax parsing, changed-line whitespace checks, and source
inspection of reset ordering, destination mapping, and phase selection passed.
Compilation and tests were not run. Renderer identity/dispatch still depends on
the source AST, and the native/renderer value paths remain separate.

Finding 2 unified application follow-up: renderer values now enter the same keyed
prepared list as native values. Their canonical PropertyKey is captured during
preparation, including page-break aliases, and renderer application no longer
uses RendererPropertyId. The separate renderer storage pool, resolved-value box,
and declaration-view field are removed. compute_declaration accepts only prepared
values and selected targets, so the CSS AST is no longer an application input.
Compound renderer values retain their per-component selection checks; native
font-size/line-height retain parent-relative bases. Renderer viewport requirements
flow through the shared value list. Unused Clone/Debug derives on the outer value
enum were removed instead of imposing them on all renderer payloads. Production
delta: +49/-78 (-29), excluding documentation; cumulative finding 2 production
delta is -273. Rustfmt syntax parsing, changed-line whitespace, key/alias coverage,
caller inspection, and source-boundary audits passed. Existing test bodies were
unchanged; compilation and tests were not run. The value enum still nests renderer,
typography, and grid payload families, carries legacy native naming, and uses an
optional key for compound white-space; full single-longhand normalization remains
open. Earlier selection and substitution stages still use parser ASTs.

Finding 2 white-space normalization follow-up: preparation splits native
white-space into keyed WhiteSpaceCollapse and TextWrapMode values, reusing their
renderer application behavior. The compound native enum/application branch is
removed. Every executable prepared value now has a required PropertyKey; native
application and physical-side resolution no longer unwrap optional identities.
All accepted native property arms were compared with the catalogue, including
word-wrap canonicalization and raw z-index/flex-basis fallbacks. Collapse is
prepared before wrapping, matching the previous branch; each destination remains
independently selected. Production delta: +89/-95 (-6), excluding documentation;
cumulative finding 2 production delta is -279. Rustfmt syntax parsing,
changed-line whitespace, catalogue coverage, and caller inspection passed.
Compilation and tests were not run. Compound renderer shorthands and legacy native
naming remain; the full single-longhand representation is still unfinished.

Finding 2 renderer shorthand follow-up: preparation expands text-wrap,
font-variant, font-synthesis, and text-box into independently keyed values in
their previous application order. Font-synthesis longhands retain only their
applicable optional component; font-variant caps reuse the Features payload.
All executable values now pass the same selected-key/phase gate. The renderer
exception and selected callback plumbing are removed from application; parsed
compound payloads are rejected as unreachable there after normalization.
Production delta: +53/-66 (-13), excluding documentation; cumulative finding 2
production delta is -292. Rustfmt syntax parsing, changed-line whitespace,
compound destination coverage, and caller/selection source checks passed.
Compilation and tests were not run. The application representation still has
nested family enums, legacy native naming, and parser-only compound variants;
finding 2 is not yet declared complete.

Finding 2 direct renderer preparation follow-up: renderer parsing now emits keyed
longhands through a callback instead of returning a temporary shorthand payload.
Text-wrap, font-variant, font-synthesis, and text-box emit only their final component
values. Font-synthesis validates both components before emitting either, retaining
atomic rejection. Its two longhands have direct scalar variants instead of
optional-component records. The second expansion match and application-time
unreachable shorthand branch are deleted. Preparation reports whether renderer
parsing handled the declaration; native values continue through the same admission
checks and native preparation path. Production delta: +58/-98 (-40); mechanical
existing-test callback adaptations: +6/-5 (+1); total +64/-103 (-39), excluding
documentation. Cumulative finding 2 production delta is -332. Rustfmt syntax
parsing, changed-line whitespace, caller inspection, and compound-variant/atomic
emission source checks passed. Compilation and tests were not run. Family payload
enums and legacy native naming still need final representation review before
finding 2 is declared complete.

Finding 2 single-value-enum follow-up: renderer payload variants now belong to the
shared PreparedValue enum, and their application arms are in its existing apply
match. The Renderer wrapper, PreparedRendererValue enum, duplicate viewport query,
and separate properties/unparsed.rs application file are removed. Font features
share one payload and destination match across native caps, renderer declarations,
and shorthand resets. PreparedLonghand and PreparedDeclarationValues replace
native-only type names; stored/resolved/view fields are named values. Renderer
syntax parsing remains a separate module implementing preparation methods on the
shared value type. The broader caller audit also found and fixed a stale
prepared.renderer containment-support query left by the earlier pool removal.
Production delta: +302/-339 (-37); mechanical existing-test renames: +6/-6 (0);
total +308/-345 (-37), excluding documentation. Cumulative finding 2 production
delta is -369. Rustfmt syntax parsing and changed-line whitespace checks passed.
Source audits covered all 67 value-application arms, moved renderer application
(normalized source comparison excluding the deliberate feature consolidation),
legacy-name/field removal, support-query consumption, and callers. Compilation and
tests were not run. Grid/typography payload families still have their own dispatch,
and the review's proposed single key-driven match still needs a final architecture
audit; finding 2 is not yet declared complete.

Finding 2 shared spacing follow-up: native letter/word-spacing preparation now
produces the shared Spacing payload, removing the duplicate typography variant,
viewport branch, and application arm. The common evaluator preserves renderer
percentage fractions through TextSpacing::new and native ex/ch/cap components
through the existing font-relative spacing interner. Native preparation still
uses LengthMode::Spacing with a length-only input; renderer preparation still
uses LengthMode::Scalar with percentage input. Thus native metric-relative
components have no percentage fraction, and renderer scalar components have no
metric-relative axes. Production delta: +22/-39 (-17), excluding documentation;
cumulative finding 2 production delta is -386. Rustfmt syntax parsing,
changed-line whitespace, preparation-mode inspection, constructor/interner
inspection, and caller checks passed. Compilation and tests were not run.
Remaining typography/grid payload dispatch still requires architecture review.

Finding 2 audit checkpoint: a shared keyed value list, one preparation entry for
stored/substituted/queried declarations, and AST-free application are present.
The review's literal single key-driven dispatch remains open; nested typography
and grid payload dispatch are still present. No further flattening was performed
merely to move code between modules. Finding 2 remains partial, with -386 recorded
production lines cumulatively.

Finding 15 normalization storage: NormalizedText now owns run ranges and deferred
control pairs directly. The Characters/Runs switch, character remap, wrapper
accessors, and source indices used only by dense mapping are removed. Range
updates retain the previous sparse algorithm; deferred controls retain source
order and offsets whether pending whitespace survives or is discarded. usize
pairs preserve run/source indices without the previous compact-index admission
check. Production delta: +17/-121 (-104), excluding documentation. Rustfmt syntax
parsing, changed-line whitespace, source diff/caller inspection, and endpoint/
pending-control review passed. No tests were added or modified; compilation,
tests, and benchmarks were not run. An earlier pipeline review identifies the
dense form's memory advantage for fragmented text, but the searched review and
benchmark records contain no measurement validating that extra form. The chosen
sparse-only representation follows this review's recommendation; it can use more
memory for highly fragmented text, and deferred control pairs are wider on 64-bit
platforms. The other storage forms listed in finding 15 remain open.


Finding 2 completion: typography and grid now prepare variants of the shared
PreparedValue enum directly. Their wrapper enums, separate application functions,
and duplicate viewport dispatch are removed. One application match receives the
canonical PropertyKey and typed payload; shared numeric/color payloads retain
local destination selection without repeating their key lists. This implements
the review's single-dispatch intent without duplicating evaluation for each key.
The AST remains in parsing, selection, and substitution, but is not an application
input. Stored declarations, substituted declarations, and capability queries use
PreparedDeclarationValues::prepare. White-space and renderer shorthands emit final
longhands; font reset values use the same selected-key/phase gate. Physical-side
selection uses PropertyKey. The earlier special application branches, family
routing, and ApplyResult are gone. Finding 2 is implementation-complete; this
supersedes its earlier partial checkpoints.

Production delta for this completion: +434/-427 (+7); mechanical existing grid-test
adaptations: +5/-5 (0); total +439/-432 (+7), excluding documentation. Cumulative
finding 2 production delta is -379. The small net increase introduces explicit
key/payload routing while removing the remaining family wrappers. Counts compare
/tmp/htmlengine-finish-prepared-dispatch-before with the completed source.
Source checks covered preparation/application destinations and aliases, shared
preparation callers, viewport dependencies, helper visibility, and removal of old
family names. Normalized source comparison preserved all 17 moved typography/grid
application bodies and all 66 previous shared application bodies; only the two
family-forwarding arms disappeared. Rustfmt syntax parsing of temporary copies
and changed-line whitespace checks passed. No tests or compilation were run, so
type checking and runtime verification remain outstanding.


Finding 2 dispatcher simplification: match the prepared value directly instead
of repeating its property key in every outer pattern. The canonical key remains
available for shared payload destinations (spacing, features, grid axes, sizes,
colors, sides, and alignment). This removes redundant key lists and the outer
mismatch panic; the value match is exhaustive. All 83 value variants have arms.
The unified representation, AST-free application, and shared preparation path
remain intact. This supersedes the previous completion note's key/payload tuple
match; the implementation uses one value dispatcher with local key selection,
rather than the review's literal single match on key.
Production delta: +84/-133 (-49); cumulative finding 2 production delta
is -428 lines. No tests were changed. Counts use
/tmp/htmlengine-simplify-prepared-dispatch-before. Source comparison of all 83
application bodies passed after normalizing formatting and expression-arm braces;
variant coverage, temporary-copy rustfmt syntax parsing, and changed-line
whitespace checks passed. Compilation and tests were not run.


Finding 11 completion audit and simplification: the resolver's RuleMatcher calls
BoundSelectors::matches_selector for local, element, and pseudo candidates. That
boolean entry owns the compact match and AST fallback, including scope, pseudo
target, and the candidate's proven bucket component. Compiled candidate matching
now combines traversal-summary matching and bucket skipping in one private
matches_candidate method; the extra matches_indexed forwarding layer is removed.
The existing unindexed compact API remains for direct comparisons that have not
proved a candidate bucket. Both selector engines remain, as the review recommends.
Finding 11 is implementation-complete at source level.
Production delta: +13/-27 (-14); cumulative finding 11 delta is
-3 lines, excluding documentation. No tests were changed. Counts compare
/tmp/htmlengine-finish-selector-entry-before with this patch. Source comparison
confirmed that the combined method is exactly the former traversal method with
its indexed callee inlined, ignoring whitespace. Caller audit, temporary-copy
rustfmt syntax parsing, and changed-line whitespace checks passed. Compilation
and tests were not run; runtime verification remains outstanding.


Finding 12 completion audit: both style-sharing paths use element_is_shareable,
with presentational hints collected before either probe and included in
has_element_declarations. The old independent hint-attribute list is absent;
language and ancestor-dependent table-cell exclusions remain explicit. Early
sharing now passes has_element_declarations to the shared predicate instead of
checking it separately and then passing false. Its early cache lookups are
read-only, and every former rejection remains a rejection. CustomCascadeCache
uses DeclarationId plus CascadeBoundary, checks parent-map identity and exact
ordered declaration equality after hash lookup, and indexes through CacheIndex.
No property_address or CachedRuleMatches references remain in the style crate.
The implementation recommendations are complete. The review's separate question
of whether both sharing caches pay for themselves remains unmeasured; neither
cache was removed without that evidence.
Production delta: +1/-4 (-3); cumulative finding 12 delta is -3 lines, excluding
documentation. Snapshot: /tmp/htmlengine-finish-cache-consolidation-before.
No tests were changed. Source/caller audits, temporary-copy rustfmt syntax parsing,
and changed-line whitespace checks passed. Compilation, tests, and benchmarks
were not run.


Finding 13 phase-finalization audit: enforce_reader_minimum is called once after
prerequisite selection, immediately before finish_prerequisites. Removed its
extra rebind_line_height call and inherited-unitless multiplication: the mandatory
phase finalizer already rebinds both authored and inherited unitless line heights
using the final font size and finite saturation. Its parent/root resolution bases
do not depend on the removed intermediate text update. The minimum still clears
deferred font-size contributions, and the scale stays in the cascade for relative
inheritance. No declaration can run between the minimum and final rebinding.
Production delta: +0/-14 (-14); cumulative finding 13 delta is
-20 lines, excluding documentation. Snapshot:
/tmp/htmlengine-reader-minimum-finalization-before. Existing tests are unchanged.
Source audit covered call ordering, authored/inherited line heights, parent/root
bases, and finite saturation; temporary-copy rustfmt syntax parsing and whitespace
checks passed. Compilation and tests were not run.
Finding 13 remains dependent on findings 1 and 5 for the single reader overlay:
the builder still applies reader adjustments after cascading, including minimum
handling for stored fonts. This patch finishes the duplicate phase-finalization
cleanup, not the broader overlay migration. The next substantive work is that
shared used-style/reader-policy design, rather than another minimum-only patch.


Findings 1 / 15 metric-storage prerequisite: BoxFontMetrics now has one indexed
representation instead of NotRequired/Dense/Indexed; an empty index vector covers
the no-metrics case. RequiredFontMetrics is removed. Box and non-box lookups both
refer to one metric table, populated once per existing font/language/feature cache
key. The cache returns IDs, eliminating the linear search through distinct metric
values. Non-box styles retain IDs rather than copies of metric records. Selection
of boxes/non-box styles requiring metrics and root metric calculation are unchanged.
The cache's existing key is retained, including size and shaping settings. Equal
metric results from different keys may now occupy separate entries; no memory or
performance improvement is claimed without measurement. Missing metric bindings
return the existing fallback metrics.
Production delta: +47/-90 (-43); mechanical existing-test constructor/import
adaptations: +10/-10 (0); total +57/-100 (-43), excluding documentation. Snapshot:
/tmp/htmlengine-unified-metric-storage-before. Source comparisons confirmed the
cache key, metric probe calculation, and root metric resolution are unchanged.
Constructor/caller and legacy-type audits, temporary-copy rustfmt syntax parsing
of all four changed files, and changed-line whitespace checks passed. Compilation,
tests, and benchmarks were not run.
This completes the multi-form metric-storage cleanup, not finding 1 as a whole.
A retained used-style store, removal of lazy resolution/per-pass snapshots, and
reader-overlay consolidation remain open. Finding 15 also retains its other
listed storage forms. No completion claim is made for either whole finding.


Finding 1 used-style resolution sharing: PassContext now resolves a used view
inside its existing geometry cache, once per (StyleIndices, metric ID), rather
than before the lookup for every box. The cached result includes the view, size
index, and box-model index. Metric IDs include the full selected metric record,
including normal line height; zero keeps fallback-metric boxes distinct. A style
handle alone is not yet sufficient because shaping selectively requests metrics.
The old three-ratio key could share geometry but was insufficient for sharing a
complete used view. Paint's standalone resolver now returns just the view; its
unused style-handle return is removed. Per-box output order and percentage-height
candidate collection are retained.
Production delta: +33/-30 (+3), excluding documentation; tests unchanged.
Snapshot: /tmp/htmlengine-share-used-resolution-before. Caller/key/data-flow audit,
temporary-copy rustfmt syntax parsing, and changed-line whitespace checks passed.
Compilation, tests, and benchmarks were not run. Different metric IDs with equal
ratios can now retain separate geometry snapshots; the cache also retains a view
per distinct key. No memory/performance claim has been measured. This remains a
per-pass cache: a retained store shared across layout and paint, lazy-accessor
removal, and reader-overlay consolidation are still outstanding for finding 1.


Finding 1 paint resolution follow-up: repaint now resolves each distinct
(StyleIndices, metric ID) once, retaining per-box view order for paint readers.
Palette overlays still occur in PaintReader, after geometry-style resolution.
The standalone resolve_box_style helper and its re-export are removed now that
layout and paint each resolve inside their cache misses. Zero metric IDs keep
fallback views separate from measured views, as in layout. Root-relative inputs
and inherited-spacing origins remain fixed within each ShapedFontMetrics input.
Production delta: +18/-18 (+0), excluding documentation; tests unchanged.
Snapshot: /tmp/htmlengine-paint-style-sharing-before. Source audit covered cache
identity, box order, default-style fallback, palette overlay placement, and the
removed helper's callers. Temporary-copy rustfmt syntax parsing and changed-line
whitespace checks passed. Compilation, tests, and benchmarks were not run.
The paint cache is local to a replay and adds temporary hash-map storage. This
removes repeated per-box resolution within replay, not rebuilding between passes.
Finding 1 remains open: retaining owned resolved values across layout and paint
requires replacing the current borrowed-view storage; lazy accessor resolution
and reader-overlay consolidation also remain outstanding.
