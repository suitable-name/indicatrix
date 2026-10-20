# Meet-preserving Optimize and material Retarget: implementation plan

Status: proposed implementation, based on source inspected on 2026-10-18. This document changes no application behaviour. No Cargo commands, builds, tests, Clippy, benchmarks or git commands were run. Verification described here is for the owner to execute. Both AI crates are outside this plan.

## 1. Required behaviour

An optimization result may move a meet point, but must not separate the facets that originally met there. Optical quality is optimized only inside this feasible set. A preservation failure cannot be traded against a higher optical score or hidden by a shape penalty.

| Requirement | Material Retarget, including Shift and Optimize | Ordinary Optimize |
|---|---|---|
| Keep every original facet junction connected | Mandatory | Mandatory |
| Keep original live facets and protected edges | Mandatory | Mandatory |
| Keep tier/index identities, symmetry and authored relations | Mandatory | Mandatory |
| Respect explicit dimensional locks and search bounds | Mandatory | Mandatory |
| Preserve general shape | Tight shape bounds; select alternatives by optics versus shape change | Broader permitted proportions; optics is the main ranking criterion |
| Allow additional meets | Only if all protected original connections survive | Same rule; new contacts are welcome within that rule |
| No feasible useful candidate | Explain why; never offer a damaging replacement | Explain why; never offer a damaging replacement |

The guarantee concerns the design at the start of this operation. Do not progressively replace that baseline with each accepted candidate, a Retarget Shift seed, or a repaired intermediate design. A later optimization started after Apply captures the newly accepted design as its own baseline.

Additional meets are permitted, not promised: with a fixed set of planar facets and non-collapsing original edges, many new contacts are geometrically impossible without altering the old design. Do not relax an old junction to manufacture extra candidates. Inserting new tiers/facets is not part of this work.

## 2. Source findings and implementation locations

Paths in this document are repository-relative. New module and type names below are proposals, not existing APIs.

| Existing location | Current limitation | Required integration |
|---|---|---|
| `crates/indicatrix-cut-core/src/optimize/candidate.rs`, `evaluate_candidate`, `SearchContext` | Checks solve, closure, optional shape guard and warning-count regression; no original incidence checks | Shared baseline, exact candidate construction, hard preservation validation before optics |
| `crates/indicatrix-cut-core/src/optimize/guard.rs` | Girdle/table guards protect selected global properties | Keep existing guards and add the independent meet guard |
| `crates/indicatrix-cut-core/src/design/hinge.rs`, `tier_hinges_in` | One hinge from the first live facet of a tier | Use hinges as optional seeds only; evaluate all facet instances |
| `crates/indicatrix-cut-core/src/optimize/space.rs`, `SearchSpace::write_angle` | Independent angle update; anchored mast follows one hinge | Coordinated, constrained candidate parameterization |
| `crates/indicatrix-cut-core/src/optimize/pool.rs`, `PoolEntry` | Stores angles and score only | Retain the entire accepted parameter state and validation identity |
| `crates/indicatrix-cut-core/src/optimize/search.rs`, `search/stages.rs`, `search/result.rs`, `search/multi.rs`, `polish.rs` | Several stages reconstruct candidates from angle vectors | Preserve the actual accepted state through every stage and final scoring |
| `crates/indicatrix-cut-core/src/optimize/apply.rs` | Atomic edits and stale angle/mast checks, without original topology validation | Trial-apply, solve and validate before committing one History edit |
| `crates/indicatrix-editor/src/optimize_view/candidate_apply.rs` | Preview and Apply reconstruct edits independently | One shared candidate materialization path |
| `crates/indicatrix-editor/src/retarget/validity.rs`, `judge` | Girdle, flat-side, tier survival and undersized checks | Original-junction and edge validation with useful failure reasons |
| `crates/indicatrix-editor/src/retarget/anchors.rs`, `refit.rs` | One-point anchoring and area-preserving flat refit do not preserve all meets | Seed suggestions or constrained adjustments; no unchecked final changes |
| `crates/indicatrix-editor/src/retarget/search.rs` | Derives search hinges after choosing a shifted start; final processing changes geometry again | Immutable original baseline; validate seeds and final processed geometry |
| `crates/indicatrix-editor/src/retarget/check.rs`, `plan.rs`, `mod.rs` | Desktop check and legacy Shift/Optimize paths differ | Shared policy across all supported entry points |
| `apps/indicatrix-cut/src/gui/editor/callbacks/retarget_actions/check_run.rs`, `apply.rs` | Idle/unchecked paths can permit Apply | Require a current, successfully validated candidate |
| `apps/indicatrix-web/src/editor/optimize/mod.rs`, editor session Optimize/Retarget APIs | Legacy angle-only interfaces | Migrate callers or explicitly reject results they cannot represent |

Other relevant existing foundations are `Design::tier_id_at`, `Design::solve_with`, `Design::planes_from_solved`, `tier_plane_ranges`, the native design-file conversions, and `Edit::Batch`/History. Preserve the History-only mutation model.

## 3. Architecture and data contracts

Add a shared module under `crates/indicatrix-cut-core/src/meet_preservation/`, exported through the crate's normal public API. It must not depend on desktop UI or on `indicatrix-editor`.

Suggested split, keeping each changed or new Rust file below 800 lines:

- `mod.rs`: public contracts and policy types.
- `provenance.rs`: stable authored-facet identity and plane provenance.
- `baseline.rs`: original geometry, junctions, edges and shape measurements.
- `validate.rs`: candidate correspondence and preservation checks.
- `diagnostics.rs`: structured violations and aggregate statistics.
- Optimizer-specific parameterization/projection goes under `optimize/constrained/`, separate from the reusable validator.

Proposed concepts:

- `FacetKey`: stable `TierId`, individual index-instance identity and surface kind. Gear, reference angle, cheater offsets and index membership are part of the geometry identity. Do not key facets by mutable row number, display name or merged plane offset.
- `SurfaceProvenance`: mapping between emitted geometric planes and all authored facet identities they represent, including duplicate/coplanar aliases. Distinguish preform boundaries from authored cut facets.
- `MeetPreservationBaseline`: immutable original design/geometry identity, original facet support sets at junctions, protected edges, face ordering, tolerances and starting shape measures.
- `PreservationPolicy`: mandatory meet protection plus operation-specific shape/motion restrictions. Product entry points must not expose a silent default that disables meet protection.
- `CandidateState`: all actual editable parameters changed by the candidate, resolved geometry, shape measures and its baseline identity. The memory representation can share immutable structures; it must not reduce the state back to angles alone.
- `ValidatedCandidate`: private-constructor wrapper around a materialized, validated state. Its validation records are meaningful only for that exact geometry and baseline.
- `PreservationViolation`: typed reasons such as detached junction, lost facet, collapsed edge, changed ordering, conflicting lock, unsupported geometry, ambiguous baseline or stale candidate.

Keep geometric validation separate from optical scoring. A geometry-only validator should be reusable by Optimize, Retarget, preview and Apply without running rays.

## 4. Work package A: extract a trustworthy original meet network

1. Capture a consistent design snapshot and resolved geometry before any Shift, hinge adjustment, table/culet refit or search. Use the same inputs as the displayed solved design; an outdated cached mesh must not become the baseline.
2. Reject cancellation, unresolved/failed solve states, non-finite values and incomplete geometry explicitly. `Design::solve` has legacy fallback behaviour at its plane limit; use the checked/cancellable path and inspect solve status rather than treating every `Ok` as a valid baseline.
3. Add provenance alongside the existing plane-emission path. Audit deduplication in schedule/plane construction and in the mesh before using `tier_plane_ranges` as an identity map. Include every index instance of every tier.
4. Build junctions from the final mesh rings and their incident surfaces. Do not derive the network from triangulated `SolidMesh.positions`, which duplicates boundary vertices and contains triangulation details. Merge numerical duplicates only when geometric residuals and provenance agree. Record the full incident-facet set, including junctions where four or more facets meet.
5. Record edges, endpoints and cyclic face-boundary order. Use original geometry to distinguish separate nearby junctions. Pairwise adjacency or equal counts alone are not sufficient.
6. Represent any active preform-boundary support explicitly. Do not mistake an artificial clipping boundary for an authored facet; preserve its actual containment/lock semantics and identify designs whose baseline cannot be established reliably.
7. Store numerical tolerances in normalized design units, with reported physical-unit equivalents where possible. Separate incidence residual tolerance from minimum usable facet area/edge length. Audit current mesh tolerances; do not blindly inherit a visual mesh weld threshold as proof of an exact meet. Validate the production planes as well as any ideal analytic projection: `planes_from_solved` passes through `GpuFacetPlane`, so precision loss in the real conversion must not be omitted from validation.
8. Treat an ambiguous near-degenerate starting junction as an explicit baseline diagnostic. Do not silently repair the design, broaden tolerances until it passes, or invent an original connection.

Imported `imported_meet` metadata remains advisory/adoptable instruction data. Preserve the actual imported geometry. Do not automatically adopt all imported meets or turn every constraint into `MeetExisting`.

Acceptance: independently identify a split multi-facet junction even when the stone stays closed and facet counts, areas and global dimensions barely change; correctly identify every member of a symmetric tier and duplicated plane aliases.

## 5. Work package B: enforce preservation before and after search

For each candidate:

1. Match facets using provenance and baseline identity.
2. Match each original junction to a candidate junction containing its complete original incident-facet set. Additional support surfaces may be present. Correspondence must be unambiguous and injective for protected distinct junctions.
3. Verify all incident-plane residuals at that one candidate point, membership in the actual surface boundary, and all relevant halfspaces. Comparing nearby vertex coordinates alone is insufficient.
4. Verify protected original edges remain connected and non-collapsed, with consistent endpoint and face-boundary ordering. A compatible subdivision may be accepted; it may not remove an old connection or fold a face boundary.
5. Verify original live facets survive individually, with nondegenerate area. Aggregate warning-count checks remain secondary and cannot hide replacement of one lost facet by another.
6. Apply existing manufacturing, rough containment and operation-specific shape bounds.
7. Only then evaluate optics.

Re-run this gate after final reconstruction, relation evaluation, any refit and immediately before publication/Apply. Cache a validation result only when the exact baseline, policy and geometry identities match. Full-fidelity scoring must not assume a previously accepted angle vector is still safe after reconstruction.

Missing validation is a failure, never equivalent to valid. In Retarget, remove the ability for Idle/Unchecked/error/cancelled checks to open Apply for a geometric change. Existing editing operations outside these optimizers retain their own behaviour.

Acceptance: all supported optimization and retargeting entry points reject broken meets, including legacy wrappers and hand-constructed results passed to an Apply function.

## 6. Work package C: preserve accepted state throughout the optimizer

This refactor must precede enabling coordinated projection. Otherwise the search may score a corrected stone and then retain only the uncorrected angle proposal.

1. Change candidate evaluation to return an accepted state plus scores/diagnostics, rather than acceptance with a score alone.
2. Coordinate descent must adopt that full state. A successful proposal can change several angles and masts; do not rewrite only the originally selected coordinate.
3. Update screening, multistart seeds, restart waves and pool merging to retain complete states tied to the same original baseline.
4. Wrap or adapt simplex polishing so the evaluated/projected state is the state returned to the caller. Its raw vector cannot stand in for the accepted state.
5. Extend candidate-pool distinctness beyond angle separation: normalized mast changes and geometric/shape separation can distinguish meaningful alternatives. Keep deterministic tie-breaking, bounded memory and pairwise deduplication.
6. In `search/result.rs`, validate and Full-score the final accepted states; do not recreate rivals through angle-only `apply_free_angles` unless exact state reproduction has been established.
7. Ensure result construction can emit every actual editable change, including a permitted mast-only change. The current `MastChange` contract assumes an angle change accompanies it; revise that assumption and its callers.
8. Audit the old `OptimizeOutcome` interface. Migrate product callers to the richer result. A compatibility wrapper may return an angle-only result only when that representation reproduces the validated state; it must not discard required mast changes.

Acceptance: retained, previewed, Full-scored and applied geometry agree for a case where one proposal causes multiple angles and depths to move, and for a depth-only permitted correction.

## 7. Work package D: generate useful feasible candidates

### Variables and constraints

For each protected junction j and incident facet f, enforce:

```text
n_f(theta) dot vertex_j = offset_f
```

Use the existing plane convention and transforms. Include each facet instance's azimuth/cheater/reference orientation and the existing conversion from tier mast to plane offset. A shared tier remains shared; do not create per-facet depth freedom that the design model cannot express.

Variables are permitted independent tier angles, permitted pinned-tier masts and auxiliary junction positions. Relation-driven angles, derived depths, dimensional targets and explicitly locked parameters are dependent values or constraints, not extra independent freedoms. Zero/near-vertical exclusions, crown/pavilion side, rough bounds, symmetry and angle bounds remain enforced.

Fix scale/translation freedoms using the existing design size, preform and lock conventions. Otherwise a projection can satisfy contacts by arbitrarily shrinking or translating the whole stone. Work in normalized coordinates and report rank deficiency rather than trusting an ill-conditioned solution.

### Projection and search procedure

1. Build the constraint graph once and divide it into connected variable blocks where this saves work.
2. Compute a local feasible direction using a rank-revealing constraint Jacobian/nullspace or equivalent equality-constrained trust-region solve. Use coupled directions across dependent tiers instead of relying solely on one-angle-at-a-time moves.
3. Propose a bounded step in a feasible direction and project the resulting angles/depths/junctions back onto the nonlinear constraints. At fixed normals, the junction/offset equality subproblem is linear; active bounds and nondegeneracy conditions still require handling.
4. If the fixed-angle problem is infeasible, adjust the angles jointly within the trust region or reject. Do not assume moving masts alone can repair arbitrary angles.
5. Materialize the candidate through the real design/edit/solve path and run the authoritative validator. Projection residuals are not a substitute for validation of the actual resulting stone.
6. Only a feasible, representable candidate receives an optical score. Use smaller steps or another feasible block after rejection; bound attempts and honour cancellation.
7. Integrate projected multistarts and polishing. Derive deterministic seeds and deterministic merge order independently of thread completion order.

Use an identity deformation and, where expressible under the locks, a common axial/radial affine stretch as initial feasible families. Transform all affected planes consistently, including their normalized offsets. Independent pavilion shifts or separate unconstrained crown/pavilion stretches are not automatically topology-preserving.

Use existing numerical facilities where suitable; choose any additional linear algebra dependency only after checking repository capabilities and conditioning requirements. Do not add an AI dependency or a new generic global optimization framework for this fix.

### Persistability boundary for the first implementation

Keep authored constraint types intact. A projected non-`ScaleReference` mast is an auxiliary target, not a value the current `MastChange` can persist. Accept it only when the final candidate's ordinary `Design::solve_with` reproduces the required geometry from its editable parameters and existing constraints. Model this reproduction inside candidate feasibility, not just as a late UI check.

This deliberately limits the search to designs the application can actually save and reconstruct. It avoids silently pinning every tier or adding an untracked solved-mast cache. If this leaves insufficient freedom in representative meet-derived designs, implement the exact-binding extension in section 11 before claiming those cases are supported; never bypass reproduction to make a benchmark improve.

Acceptance: demonstrate coordinated nonzero feasible motion and useful candidates for both imported pinned and meet-derived fixtures, preserve locks/relations, and terminate cleanly when the valid design has no remaining degrees of freedom.

## 8. Work package E: make Retarget preserve the original design

1. Capture the original baseline once in the Retarget request before `choose_start` or any Shift. Pass it through `check`, `search`, final candidate processing and Apply.
2. Keep `build_plan` cheap: its angle rows describe requested changes, not a certified applicable result. Shift's critical-angle targets become a preferred deformation to project onto the feasible set, rather than mandatory independent edits that can break contacts.
3. Replace one-point follower/neighbor anchoring with the constrained candidate path. Existing hinge and flat-area calculations may supply initial guesses or shape targets.
4. Integrate table/culet preservation into the same constraints. Remove independent unchecked postprocessing. If refitting still runs separately, it must project jointly and pass the full final gate before scoring.
5. Preserve the original baseline across partial-shift attempts, unchanged seeds and all alternatives. Never validate only against the best shifted start. Keep it separate from mutable working analyses such as `StoneAnalysis::split_at_girdle`, which changes reference geometry for Retarget's calculations.
6. Carry target material, physical size, lighting and objective identity with the request. Score the unchanged geometry under the target material as the fair optical baseline. Source-material performance can be shown separately.
7. Offer a small nondominated set of feasible alternatives balancing optical improvement against normalized shape change. Suggested labels: Closest to original, Balanced, Best optics within limits. Do not populate three slots with effectively identical candidates.
8. Require a documented optical improvement floor for an 'improved' claim. A material-only unchanged-geometry result can still be offered explicitly; do not relabel it as a successful geometric optimization.

### Shape policy

Use hard bounds for the design properties the user intends to retain, and shape distance to rank the feasible remainder. Metrics should include girdle outline/aspect, table size, normalized crown/pavilion depths, girdle thickness and junction displacement after removing permitted global scale/translation. Current table-percent and crown/pavilion-ratio penalties alone are insufficient.

Initial conservative policy: preserve index/azimuth/symmetry exactly; preserve an explicitly fixed girdle/outline exactly; retain existing manufacturing floors; use explicit finite bounds for remaining proportions. Put defaults and presets in one policy type with units and explanations. Calibrate numerical default bounds against the owner's representative corpus before release; no percentages have been experimentally justified by this analysis. Large changes that exceed the selected bounds produce no candidate, not automatic bound widening.

For ordinary Optimize, retain the same hard meet constraints while allowing its existing requested angle ranges and broader proportion changes. Shape drift can break ties or distinguish alternatives after validity, never buy permission to detach a meet.

Acceptance: an initial Shift that breaks a meet cannot become a valid search baseline; all displayed alternatives pass preservation and final target-material scoring.

## 9. Work package F: preview, Apply, undo and persistence

1. Implement one materialization function used by preview and Apply. Apply edits to a scratch design, evaluate relations/targets, solve, validate against the original baseline and compare with the candidate's recorded geometry.
2. Check stable identities and a deterministic fingerprint of every input affecting geometry/feasibility: tier membership/angles/constraints, indices, gear/reference/cheaters, relations/targets, preform/size and policy. Also track material/objective/lighting for score freshness. Same old angle at the same row is not enough.
3. Perform expensive revalidation on the worker if necessary, then check the design generation again at commit. Commit the exact validated edit atomically through History, including the material change for Retarget.
4. Any stale result requires recomputation; no silent remapping to whichever tier now occupies that position. Preview construction errors must be reported rather than ignoring failed relation application or missing rows.
5. Native save/reopen must recreate the same geometry and protected network from stored authoring parameters. Use `native/design_file.rs`, conversion/load modules and their round-trip facilities to verify this; do not persist an invalidatable certificate as if it were proof after load.
6. Undo restores the original geometry/material/relations; redo restores the validated candidate. No separate hidden state may remain after either action.
7. Export uses the accepted solved schedule. Where a format rounds numbers, independently measure the exported/reimported meet error and expose unsupported precision instead of implying a stronger guarantee than the format provides. Native save/reopen is the mandatory persistence guarantee.

Acceptance: preview, Full scoring, Apply, undo/redo and native reopen agree within the declared geometry tolerance; stale/reordered-tier cases fail atomically.

## 10. Work package G: diagnostics, performance and supported geometry

User-facing diagnostics should identify the affected tiers/facets and the limiting condition: for example, 'No valid improvement found without separating Crown Main and Star at their existing junction.' Distinguish this from cancellation, invalid baseline, exhausted budget and unsupported surface type.

Expose brief candidate facts: original meets preserved, added meets if any, maximum normalized/physical meet residual, proportion changes and optical metrics. The residual is a numerical accuracy measure, not a slider for permitting detached meets. Keep detailed projection reasons and counts in diagnostic output.

Record separate time/counters for baseline extraction, projection, geometric rejection, solving, Fast optics, final validation and Full optics. Cache immutable provenance and graph structure; warm-start local projection; reuse each candidate's solved geometry across validation and scoring. Do not rebuild the graph or run extra optical evaluations just to diagnose invalid geometry.

Projection iterations must observe cancellation and explicit budgets, including candidates rejected before optics. Report both attempted candidates and optical evaluations so strict constraints cannot create an apparently stuck or unbounded search. Preserve deterministic outcomes across permitted lane counts.

The initial planar implementation must detect concave/tool geometry explicitly. Do not validate only its planar envelope and claim its real meets survived. Until exact surface provenance and contact validation exist, return an explicit unsupported-preservation result for such a design. A later curved-surface extension must support the true tool intersections and their boundary connections; this is a declared limitation, not successful coverage of concave designs.

## 11. Conditional extension: exact support bindings

This extension is required only if coordinated candidates that should be useful cannot be represented with existing authoring parameters because the generic meet solver changes branches. It is not a reason to delay the immediate hard rejection fix.

- Add strict resolved meet bindings using stable facet/junction support identities, separate from tier-name parsing and vertex ordinal heuristics. A missing bound support must fail rather than choose another vertex.
- Preserve authored instructions and dimension/angle relations. Define solver precedence explicitly; do not silently replace authored constraints with stored masts.
- Represent any exact realization/branch selection needed to recreate the accepted candidate as authored design state, with explicit invalidation/remapping for add/remove/reorder/index edits.
- Extend History and `ScheduleState` together so undo/redo restores bindings and their realized geometry.
- Extend the corresponding `indicatrix-formats` native DTO, core native conversion/load/save and format-version handling. Older readers must not silently ignore geometry-critical bindings and load a different stone.
- Require deterministic solve plus native round-trip before exposing projected results that depend on this representation.
- Keep strict binding behaviour scoped to designs carrying it; do not globally remove reconstruction heuristics from imported legacy instructions.

If this extension is not implemented, a result whose ordinary authoring state cannot reproduce its accepted geometry remains rejected, with a specific diagnostic. This is safe and narrower than claiming unrestricted optimization of every topology.

## 12. Owner-run verification matrix

Tests should exercise geometry and observable behaviour, rather than mirror the implementation. No verification commands were run while writing this plan.

| Case | Required outcome |
|---|---|
| Four/five-facet junction splits into several triple junctions; same facet count and closed solid | Candidate rejected before optics |
| Junction moves while every original incident facet remains joined | Accepted if remaining constraints pass |
| One original facet disappears while another previously absent facet appears | Rejected despite unchanged warning/live counts |
| Protected edge collapses, two protected vertices merge, boundary order changes | Rejected |
| Compatible added contact or edge subdivision | Accepted only when original junctions, edges and faces survive |
| Every index instance of a symmetric tier; asymmetric or cheater-offset fixture | All physical instances validated |
| Duplicate/coplanar emitted planes, reordered tiers, duplicate names | Provenance remains correct; stale results rejected |
| Tiny/large physical scales; nearly coincident but distinct junctions | Scale-consistent decisions without false welding |
| Fully pinned imported design with advisory imported meets | Preserves actual imported junctions without auto-adoption |
| Meet-derived design whose generic solve chooses a different branch | Rejected or solved by a supported strict binding |
| Existing dimensional targets, angle relations and explicit locks | Preserved throughout search and Apply |
| Initial Retarget Shift breaks a meet | Cannot reset the baseline; project/reject that seed |
| Flat refit or relation evaluation breaks a previously valid candidate | Final gate rejects it before publication |
| Projection changes several parameters or only a mast | Pool, polish, reconstruction and Apply retain exact accepted state |
| Desktop and legacy web/API paths | Same preservation guarantees, no angle-only truncation |
| No feasible motion; budget exhausted; cancellation | Clear status, bounded work, no damaged fallback |
| Target material versus source material | Improvements use unchanged geometry under target material |
| Preview, Apply, undo, redo, native save/reopen | Same intended geometry and meet network |
| Concave/tool geometry before support exists | Explicit unsupported result; no planar false assurance |
| Identical seeds with different lane counts | Deterministic geometry and candidate ordering |

Use small deterministic synthetic fixtures for the invariants and an owner-selected collection of real designs for feasibility, quality and timing. Include at least one real design that currently exhibits the reported damage. Capture junction residuals and connection differences as evidence; optical scores or screenshots alone cannot prove preservation.

## 13. Delivery sequence and completion criteria

1. **Safety patch: A + B + the Apply gate in F.** Capture the original network and reject damage everywhere, including final/apply paths. Continue using the current candidate generator temporarily. This may produce fewer candidates; report that honestly.
2. **State refactor: C + shared materialization in F.** Carry exact accepted state through coordinate, multistart, pool, polish and result paths before adding coupled moves.
3. **Feasible search: D.** Add bounded projection and coordinated moves; prove candidates persist through the existing design representation. Implement section 11 if the supported-design acceptance cases require it.
4. **Retarget integration: E.** Project Shift, replace unchecked refits, preserve the original baseline and rank feasible shape/optics alternatives. Cover legacy entry points in the same delivery.
5. **Completion: remaining F + G + owner verification.** Native round-trip, undo, diagnostics, cancellation, deterministic parallelism and corpus quality checks.

Baseline extraction/validation and the candidate-state refactor can be developed in parallel once their interfaces are agreed. UI/persistence changes depend on the shared candidate contract; do not independently implement different validators in each front end.

The safety patch alone is not completion of the full improvement: the finished work must also demonstrate useful coordinated optimization on representative pinned and meet-derived designs. Conversely, a larger optical gain does not compensate for a single broken original meet. Unsupported or rigid designs must be identified accurately, with no synthetic success or weakened preservation rule.
