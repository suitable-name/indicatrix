# Meet-preserving optimizers: implementation handoff

Date: 2026-10-19. Implementation and review were performed directly by the parent. (Later the same day the owner ran the suites; a line below that cites "the owner's 2026-10-19 run" refers to that run, not to anything this report did.) The two AI crates were excluded. No builds, tests, Clippy, cargo, git, benchmarks or SQLite access were run. No Slint or tutorial recorder files were edited.

## Compiler feedback addressed

- `retarget/mod.rs::rows_from_outcome` now reads untouched angles from its `seeded` parameter; the undefined `design` identifier is gone.
- Removed the unused production `Edit` import (retained a documentation-only import for links).
- Candidate evaluation, pool compatibility helpers and multistart angle-vector output used only by tests are scoped to their consumers.
- Polish result fields are consumed by consistency checks.
- Removed obsolete independent Retarget refit work and unused production imports; the retained historical follower helper is test-only.
- The GPU golden capture report now displays its tolerance-failing pixel count.

These are source fixes, not a claim that compiler warnings are exhausted.

## Preservation contract

Both optimizers capture the operation's original planar geometry before Shift, seeding or search. The immutable baseline retains authored surface identities and production deduplication aliases, complete junction support sets, live facets, protected edges and oriented boundary cycles. Every accepted candidate must reproduce those connections through the ordinary authoring solver. New compatible boundary vertices are allowed; original junctions cannot split, merge or detach, and original facets/edges cannot disappear.

Optical improvement cannot override the geometry gate. Retarget uses stricter hard shape limits than general Optimize. Unsupported or unrepresentable geometry fails explicitly instead of publishing an altered meet network.

## Gaps completed in this pass

1. **Useful coordinated proposals.** Search now tries a common axial-affine deformation in both directions inside its proposal budget, followed by bounded nonlinear projection. Identity remains the already-scored starting state. Finite-difference projection retains ordinary derived constraints and relation ownership. Original junction support bases are cached for repeated residual evaluation.
2. **Authorized depth freedom.** Pinned relation followers and horizontal tiers can contribute depth variables independently of angle freedom. Explicit dimensional targets, selection masks and vertical girdle depths remain locked. Independent follower-angle edits are refused.
3. **Complete accepted-state transport.** Pools, multistart, polish, final Full scoring, rich candidates and legacy outcomes carry the accepted geometry's angle and pinned-depth changes. Portable provenance records exact native source/candidate authoring identities, scoring inputs and policy. The web protocol transports it too. Missing, stale or truncated provenance fails Apply. A rich result's winner must match its summary.
4. **One materialization path.** Preview and Apply reconstruct, settle relations, solve and validate the same state. Old preview wrappers return explicit errors instead of an unchanged design. Source freshness is separate from the original preservation baseline, so a shifted Retarget seed can retain the pre-Shift network. Session Apply keeps follower depths and derives their angles in the same undo step.
5. **Shape and accuracy diagnostics.** Hard limits cover junction displacement, aspect, table percentage, crown/pavilion depth and girdle thickness. An optional fixed-outline policy (`fixed_outline`, enabled by no preset) can hold original x/z junction positions. Both presets always hold the girdle gauge (the outline's maximum radius and the height of the junctions on it, to the incidence tolerance); that lock is now the explicit `lock_girdle_gauge` policy field and a refusal names the quantity (`GirdleGauge`). Facts include physical residuals when the design has a physical diameter, additional boundary vertices and actual shape measurements. Failures identify authored surface names where available. Retarget alternatives are labelled by position in the score-sorted frontier (index 0 is Best optics within limits, the minimum-distance entry is Closest to original, every other entry is Balanced); "Balanced" is not computed from any balance.
6. **Cancellation and timing.** A `SolveControl` cancellation predicate reaches Retarget's closure-based analysis and projection solves between solves; it does not interrupt the meet solver's own candidate enumeration (the long pole of one solve), which only honours a cancellation flag, and `analyze(.., false)` in the Retarget search is not cancellable at all. Cumulative diagnostics record baseline, projection, solve, validation, Fast optics, Full optics and final-check calls/timings, plus projection iterations/rejections. Browser wasm records counts without unsupported `std::Instant` timing. Timings do not affect candidate equality or ranking.
7. **Final publication gates.** Full scoring rechecks the preserved network, manufacturing constraints and shape guards on the exact published state. Nonfinite scores are rejected. No independent follower/table correction is applied after scoring.
8. **Export precision.** ASC/GCS export reparses the serialized text and measures its geometry against the exact solved schedule. Success reports residual facts. Regrouped, open, rounded-incompatible or otherwise uncertifiable schedules receive an explicit precision notice; export is not silently represented as exact. Native save/reopen retains authored constraints.

The original implementation's stricter Retarget frontier selection, unchanged-target-material comparison and shared checked Retarget application remain in place.

## Regression coverage authored, not run

- Split four-facet apex rejection and nonzero preserved affine motion.
- Production duplicate/cheater aliases, bound enforcement and stale membership.
- Pinned flat depth movement with all angles locked.
- Solver-level cancellation and shape/physical residual diagnostics.
- Parsed export precision and intentionally damaged export rejection.
- Rich/legacy outcome state equivalence, serialization, stale-source and tampered-depth rejection.
- Positive editor preview/Apply/undo/redo reproduction with relation followers and coordinated depths.
- A shifted parameter source with an immutable pre-Shift geometry baseline.
- Nonzero motion and native roundtrip for both pinned and meet-derived RBC fixtures, using the production affine proposal.

Older hand-built uncertified Apply fixtures were migrated to explicit rejection assertions. Positive certified reproduction coverage was added separately; a permissive arbitrary angle edit is no longer evidence that optimizer Apply is correct. Large existing fixture files were split with `include!` in their original module scope.

## Limits requiring owner validation

- Source inspection and standalone rustfmt parsing do not establish Rust name/type correctness or behavior. No passing build/test claim is made.
- Hard shape/gauge defaults are engineering defaults requiring corpus calibration. Retarget limits are 0.08 normalized junction motion, 0.1 aspect change, 8 table percentage points, 0.08 normalized crown/pavilion depth and 0.02 girdle change. Optimize allows broader changes. They are never widened automatically to obtain a score improvement. Since 2026-10-19 (loop 4) the Retarget depth, girdle and junction limits follow the travel the REQUESTED facet turn implies: see "Retarget bounds follow the turn" below.
- Concave/tool surfaces remain explicitly unsupported by this planar preservation model.
- Ordinary authored constraints may not reproduce every useful projected branch. Conditional exact-support binding/schema work from plan section 11 remains a decision for the owner-run meet-derived nonzero-motion tests and corpus. Such states currently fail closed; this is not evidence that all meet-derived designs are supported.
- Several stages still solve again for independent validation. Timings expose this cost; no performance improvement or latency bound has been measured. Cancellation of existing optical calls keeps its prior granularity.
- The portable scoring identity has an explicit comparison API. Existing product generation guards still govern live settings; generic callers must compare current scoring inputs when presenting old scores under changed settings. Geometric Apply always checks exact authoring identity and accepted state.
- Export diagnostics cannot certify a schedule that regroups surface identities or needs live preform faces. Native authoring remains the exact-state persistence path.
- Existing identity/golden outputs affected by explicit preview errors or new diagnostic columns need owner review, not automatic regeneration.

## Owner-run validation

Start with the geometry and reproduction tests:

```sh
cargo test -p indicatrix-cut-core --lib meet_preservation
cargo test -p indicatrix-cut-core --lib optimize::tests::preservation
cargo test -p indicatrix-editor --lib optimize_view::candidate_apply
```

Then run the affected suites and compile desktop GPU callers:

```sh
cargo test -p indicatrix-cut-core --lib
cargo test -p indicatrix-editor --lib
cargo test -p indicatrix-web-core --lib
cargo test -p indicatrix-cut --features gpu --no-run
```

Finally exercise the originally damaged designs, both material Retarget modes, alternative selection, native save/reopen, preview/Apply/undo/redo, cancellation and identical seeds at multiple lane counts. Check actual junction preservation and nonzero useful motion, not only score improvement. Compare timing diagnostics before choosing corpus limits.

The source manifest is `docs/meet-preserving-optimizers-changed-files.txt`. Static parse and line-count results are recorded below after the final pass.

Final static pass (as first written): standalone rustfmt parsed 89 manifest Rust files without rewriting them. This was syntax validation only, not compilation or tests, and the figures did not hold: the manifest lists 93 Rust files, the largest is `indicatrix-editor/src/retarget/check/tests.rs` at 780 lines (757 is `optimize_run.rs`), and the files were not formatted (lines over 120 columns and mis-indented lines remain in `optimize/search.rs`, `optimize/search/result.rs`, `web-core/solve/analysis.rs` and `optimize_view/candidate_apply.rs` until `cargo fmt` runs).

Compiler follow-up: fixed the Retarget validity test fixture to store `Arc<Design>`, and migrated the CLI Retarget Shift/Optimize decisions to fallible application. Shift consumes the complete validated candidate; the CLI session retains relation-owned angle handling. These follow-up changes were source-reviewed only; no builds or tests were run. The manifest now also includes `apps/indicatrix-cli/src/commands/retarget.rs`.

## Sapphire Shift projection follow-up (2026-10-19)

Source inspection found a premature rejection in the shared nonlinear projection: the previous correction required redundant finite-difference equations to agree exactly. An inconsistent linearization, including production-normal rounding, is not proof that the authored design cannot preserve its meets. `optimize/projection_step.rs` now computes column-scaled, damped least-squares corrections with reorthogonalized QR. The projector allows twelve correction iterations and checks the full validator at every iterate, rather than requiring a separate stricter residual threshold first. Trust bounds, authored constraints, meet tolerances, topology and shape validation remain unchanged; only validated candidates can be returned.

Regression coverage now checks dependent/rounded equations and locked variables, plus preservation of the original meets after the CLI sapphire retarget is saved and reopened. Editor Apply tests consume the complete checked candidate, including coupled angle corrections, and the sapphire case checks nonzero motion and native roundtrip preservation. The success expectations were not changed, but the three CLI sapphire tests are red (see "Known red tests").

The four changed Rust files were source-reviewed and parsed/formatted with standalone rustfmt; all remain below 800 lines. No builds, tests or Clippy were run. The reported three CLI failures and the editor/core projection regressions still require an owner-run confirmation; no passing runtime result is claimed.

### Follow-up after the same three CLI failures recurred

The numerical change above did not resolve the owner-run sapphire failures. Further source review identified a sequencing regression: preservation projection runs before the table/culet refit, and a projection rejection made `refit_attempt` return immediately. A shallower pavilion can leave the old culet plane beyond the apex; recovering that missing facet is an explicit purpose of the existing refitter. The equality correction alone cannot recover every such boundary change.

When the first attempt failed, `refit_attempt` now obtains geometry-only analysis of the proposal to seed the existing flat-height repair. This analysis is never an accepted result: the repaired proposal goes through `Attempt::new` again, including projection against the original baseline and the full validity gate. Successful first attempts retain their existing path, cancellation is propagated, and no tolerances or preservation rules change.

Two new unexecuted regressions cover a vanished culet despite a closed solid, and refusal of an unrelated authoring change even when geometric refitting succeeds. The sapphire CLI success, deterministic report, JSON verdict, nonzero-motion and native-roundtrip expectations were left as written; the three sapphire tests are still red (see "Known red tests"). Standalone rustfmt source parsing only; owner-run tests are still required.

### Follow-up after refitting ran but projection still failed

The next owner run still failed the same three CLI cases, with seven mast adjustments instead of five. The refit route is now reached, but does not by itself cure the projection failure. Source review found two additional gaps in the shared correction:

- Junction residuals enforced support-plane incidence but did not penalize intersections outside other facet half-spaces. They now omit the three algebraically satisfied basis equations and include the maximum positive non-support half-space violation per original junction. Row layout stays fixed and new contacts remain allowed.
- Normalizing every derivative column to unit length amplified very weak numerical directions. QR now uses explicit physical parameter scales (the existing five-degree and 0.1 normalized-mast trust limits), with damping in those units. A common step factor shortens an oversized correction to the remaining trust box before line search, preserving its coupled direction. A fully valid line-search point can be accepted even when rounding prevents a strict residual decrease.

The final meet, facet, edge, cycle, shape, gauge and authoring checks are unchanged. Projection failures now report the stage and the current iterate's actual annotated geometry rejection instead of erasing every cause as `Unrepresentable`.

Unexecuted regressions cover clipped original junctions, source self-validation, weak derivatives, remaining trust-room scaling, and preservation of the actual refusal reason. Source review and standalone rustfmt parsing only; no builds, tests or Clippy. Rerun the CLI retarget tests and the core `meet_preservation::network::tests` / `projection_step::tests` filters. Passing runtime behavior is not yet established.

## Review corrections, loop 1 (2026-10-19)

A read-only review of this work found the following; the core-crate part is fixed in this pass, the rest is listed with its owner.

- **Junction capture no longer depends on the display mesh.** Junctions are the clusters, at the preservation tolerance (`2e-6` of the stone), of the feasible plane-triple points of the f64 arrangement; faces are the junctions whose support contains a surface, ordered by angle in the face plane. The welded display mesh used to decide which plane belonged to a four-plane meet: its weld radius scales with the *rough*, the tolerance with the *stone*, so a large rough could drop a plane from a junction's support and let a damaged meet (the table lowered into a ring) pass the gate. (The fail-closed near-incidence band this pass added was replaced on 2026-10-20, see the next section.) Rough sizes 4, 64 and 512 are exercised by `meet_preservation::tests`. One cost: every validation now walks the arrangement once in addition to building the mesh.
- A failure of the *candidate* (solve, topology) is reported as `CandidateTopology`, not as "Cannot establish the original meets"; a relation refusal is `Relation`.
- The projection solves each probe once (verdict and residuals from the same solve) instead of twice or three times.
- A candidate that fails re-validation while it is stamped is dropped on its own; the result's best entry is rebuilt from the first survivor.
- `OptimizationProvenance.diagnostics` is no longer serialised (timings made identical runs differ on the wire).

### Meets are identified at `5e-5`, reproduced at `2e-6` (2026-10-20)

The near-incidence band of loop 1 refused a capture when a live facet missed a junction by one to two tolerances. A read-only census of the first 100 library designs (sapphire, shift) showed 54 such refusals, concentrated in the older catalogue series, which are GemCad 4.x files: their masts are written with five decimals, and the quantisation (half a quantum, `5e-6`) opens their designed meets by `2e-6 .. 2e-5` of the stone. GemCad 5 files (eight decimals) miss by less than `2e-7` in `f64` and by less than `2e-6` after the `f32` production planes. A miss beyond the band was captured as two or three three-valent junctions joined by edges a few millionths long, which any repair that closed the meet then broke.

The rule is replaced by two radii:

- **Identification, `5e-5` of the stone** (`IDENTIFY`, `meet_preservation/network.rs`): plane-triple points within it are one junction, a plane within it belongs to the junction's support, and the largest support miss of the original is recorded as the junction's `slack`. `5e-5` is the meet solver's own incidence scale, 0.5 micrometres on a 10 mm stone.
- **Reproduction, `2e-6` of the stone** (unchanged): a candidate must meet every original junction to the larger of the tolerance and that junction's slack. The projection's residual rows aim at the original's own miss where it exceeds the tolerance and at zero otherwise, so exact designs produce the same rows bit for bit.

There is no refused band: a plane beyond the radius is no member of the meet and is protected by its own junction. A damaged near-meet (a facet moved ten times its slack) is still refused as a detached junction. The census (`census_version` 2) reports `gemcad_version`, `mast_decimals`, `faces`, `valence`, `split_ratio` and `max_slack_ppm` per design. Tests: `meet_preservation::tests::quantised`, `network::topology::tests` and the CLI census test on the GemCad 4.51 fixture `shah.asc`.

### Retarget bounds follow the turn (2026-10-19, loop 4)

Owner run 3 left the diamond-to-sapphire check tests red with `ShapeLimit`. The anchored seed (every facet turned about its girdle outline corners) kept every original meet, the girdle gauge and the corner band; what refused it was the constant Retarget bounds. A shift between refractive indices turns the pavilion by the critical-angle difference (ten degrees from n 2.42 to 1.77, sixteen to quartz), and a facet turned about the outline carries the points it reaches by `l (tan t' - tan t)`, `l` its reach inward from the outline, up to half the stone's length: the brilliant's culet travels 0.17 of the stone and the pavilion deepens by 0.17 width units against bounds of 0.08, and the band figure, read at the ridge points inside the corners, grows by 0.024 against 0.02. No constant can hold a one-degree shift tight and admit a ten-degree one, so `PreservationPolicy` gained `bounds_follow_turn` (true for `RETARGET`, false for `OPTIMIZE`): when set, the gate adds the travel the turn implies to `max_depth_change`, `max_girdle_change` and `max_junction_displacement` before judging the shape (`PreservationPolicy::for_turn`, applied in `validate_solved`: the projection judges by the request's turn, capped at the candidate's own, and the finished-state gates, which have no request, by the candidate's own, so whatever the projection accepts they accept too). The meets, the girdle gauge, the table and the aspect are not relaxed, and the constants still bound what the projection's repair adds to the turn.

The owner tightened the rule the same day (decisions of 2026-10-19). The travel is the largest per-tier product `reach_i x |d tan_i|`, not one half-length times the largest tangent change: `reach_i` is the tier's real inward reach from the girdle outline, read once at capture in the original solved geometry (`meet_preservation::reach`: from the outline's support along the facet's azimuth to the facet's innermost ring vertex; half the stone's length is the fallback for a turned tier with no readable facet, and only then), and `d tan_i` is read from the REQUESTED angles (`tangent_changes`): the projection takes the request from its caller (`OptimizeOptions::turn_request`; the editor hands the anchored candidate's turn to both of its projections, the plain one and the refit built on the plain one's projected state) or, without one, measures it once on its seed, and hands it to every probe's assessment; the grant is never more than the candidate's own turn implies, so a repair can neither enlarge the allowance nor keep one for a turn it undid, and a finished state judged through `validate` reads the turn from its own angles. A run that granted a turned tier the half-length fallback says so in `SearchDiagnostics::turn_reach_fallbacks`. Only tiers the request turned count (a fixed tier contributes nothing; a relation follower counts with its own derived turn, since its facet really turns; the per-tier maximum never adds it to its driver's), and a tier steeper than 75 degrees has its tangent capped there (`TURN_TANGENT_CAP_DEG`: the tangent grows without bound toward the wall while a steep facet's real reach shrinks, so the linear hinge model stops describing the travel and a near-wall request would otherwise buy an unbounded allowance). The bound is never tighter than the constant. For the diamond-to-sapphire brilliant the pavilion main reaches 0.99 R inward, so the grant is 0.18 W against the 0.17 W the stone deepens by; the octahedron fixture's crown facets reach half the width, so their ten-degree turn is granted the 0.214 its apex travels. The policy travels in `OptimizationProvenance` and is re-validated on Apply, which is why the browser worker protocol moved to 22 and the network protocol to 27.

### Known red tests (as of this pass)

- The three CLI sapphire tests in `apps/indicatrix-cli` (see the follow-ups above).
- `identity_pins_continued.rs::retarget_output_is_pinned` (Windows only): the printed value types changed; the owner re-records it.
- `indicatrix-editor/src/retarget/anchors.rs` tests `an_anchor_for_a_missing_tier_is_dropped` (stale: a stale anchor is now `StaleCandidate`) and `apply_with_no_anchors_is_the_plain_angle_edit` (compares a function with itself).
- `indicatrix-editor/src/retarget/tests_part1.rs`: the `.expect("Shift mode never fails")` tests are at risk because `build_proposal` now validates the angle-only trial.
- Not run by this pass: nothing in this pass was compiled or run.
