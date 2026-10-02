//! Profile-guided-optimization training workload for `indicatrix-cut-core` itself.
//!
//! Run alongside `crates/indicatrix/examples/pgo_train/main.rs` by `scripts/pgo-build.ps1` /
//! `scripts/pgo-bolt-build.sh` inside an instrumented build -- see this crate's own
//! doc comment (`src/lib.rs`) for "the pieces" this exercises. `indicatrix`'s own
//! `pgo_train` example trains the geometry/solver/renderer crate directly; this one
//! trains the editor-core layer built on top of it: [`Design`] construction,
//! [`Design::solve`], `.asc` export, the `.indicatrix` design file save/load round
//! trip, [`resolve_after_edit`], and [`optimize_design`]'s search loop.
//!
//! For each built-in template in [`templates::TEMPLATES`] (five small designs, all
//! pinned `MeetConstraint::ScaleReference`-only -- see that module's own doc comment)
//! AND the crate's own CrackOtto-Step cost-probe `.asc` fixture (103 tiers, 205
//! facet-plane instances, the one real, heavily meet-derived design this crate ships
//! -- see `indicatrix_cut_core::optimize`'s own measured cost table for why this one
//! design dominates this whole file's runtime): `Design::solve` ->
//! `to_asc_schedule_from_solved` -> `indicatrix_formats::asc::to_asc_string` ->
//! `native::save_paired` (the catalogue's `.asc`) -> `native::design_to_string` ->
//! `native::design_from_str` -> `resolve_after_edit` for one
//! `Edit::MoveTier` -> `optimize_design` with a tiny evaluation budget.
//!
//! Every `Design::solve` call on CrackOtto-Step costs over a second on its own (no
//! incremental resolve helps here -- `optimize`'s module docs explain why), so this
//! file only ever solves it directly, never repeatedly: `optimize_design` runs
//! against it too, but without a free (non-`ScaleReference`) tier forced onto it the
//! way the templates get, so it exercises the baseline-scoring and zero-free-tiers
//! early-return code paths rather than the coordinate-search loop's own
//! repeated-solve cost -- the search loop itself gets real coverage from the
//! templates instead (each solve is still a real fraction of a second on these --
//! see `pick_adoptable_free_tier`'s own doc comment for why not every template can
//! safely be forced into the search loop either). Deterministic: every stage prints
//! a checksum folded from solved masts, exported text lengths, and search outcomes;
//! run twice, the checksums are identical. Keeps the whole run under roughly 30 s.

use indicatrix::{
    geometry::meet_solver::{self, Block, MeetConstraint},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    Design, Edit, OptimizeConfig, PreformSpec, SearchHooks, free_tier_indices,
    native::{DesignExtras, design_from_str, design_to_string},
    optimize_design, resolve_after_edit, save_paired, templates,
};
use std::time::Instant;

/// The crate's own CrackOtto-Step cost-probe fixture -- see this file's module doc
/// comment. `include_str!`'d directly (not duplicated) from the same file
/// `optimize::cost_probe`'s doc comment measures against.
const CRACKOTTO_ASC: &str = include_str!("../src/optimize_cost_probe_crackotto_step.asc");

/// Picks one tier `train_one_design` can safely adopt away from
/// `MeetConstraint::ScaleReference` so `optimize_design`'s free-tier coordinate-search
/// loop has something to move.
///
/// Not just "any non-girdle tier": every tier here starts pinned `ScaleReference`
/// (see [`templates`]'s and `Design::from_asc_schedule`'s own doc comments), and
/// `Design::solve`'s own missing-anchor check requires at least one `ScaleReference`
/// tier per crown/pavilion/girdle block that has any tiers at all (see
/// `crate::design`'s module docs, "Scale anchoring") -- freeing a block's ONLY
/// anchor makes `optimize_design`'s baseline solve fail outright instead of running
/// the search (measured against this crate's own "Simple Teaching Design" template,
/// whose crown block is exactly one tier). So this only ever picks a tier whose
/// block has at least ONE OTHER `ScaleReference` tier left to anchor it, using
/// [`meet_solver::classify_blocks`] (the same classification `Design::solve` itself
/// checks against) rather than guessing from `angle_deg` alone. `None` when no tier
/// qualifies (e.g. every block here has exactly one tier) -- the caller leaves
/// `design` untouched and `optimize_design` takes its zero-free-tiers early return,
/// same as it does for CrackOtto-Step.
fn pick_adoptable_free_tier(design: &Design) -> Option<usize> {
    let inputs = design.meet_tier_inputs();
    let blocks = meet_solver::classify_blocks(&inputs);
    let anchor_count = |target: Block| {
        inputs
            .iter()
            .zip(&blocks)
            .filter(|(t, b)| {
                **b == target && matches!(t.constraint, MeetConstraint::ScaleReference(_))
            })
            .count()
    };
    let counts = [
        (Block::Crown, anchor_count(Block::Crown)),
        (Block::Pavilion, anchor_count(Block::Pavilion)),
        (Block::Girdle, anchor_count(Block::Girdle)),
    ];
    inputs.iter().zip(&blocks).position(|(t, b)| {
        matches!(t.constraint, MeetConstraint::ScaleReference(_))
            && t.angle_deg.abs() <= 89.5
            && counts.iter().any(|&(block, n)| block == *b && n >= 2)
    })
}

/// Runs the full pipeline (see this file's module doc comment) against one design,
/// folding a checksum of everything it touches into `checksum`.
///
/// `original_asc` is the real `.asc` text `design` was imported from, if any (for
/// `save_paired`'s preserve-original-text path); `None` for a template built
/// straight from [`templates::TemplateSpec::tiers`], which never had one.
///
/// `force_free_tier`, when `true`, adopts one tier's constraint away from
/// `ScaleReference` before calling [`optimize_design`] so its coordinate-search loop
/// actually runs (every template and every `Design::from_asc_schedule` import pins
/// every tier to `ScaleReference` -- see [`templates`]'s and
/// [`Design::from_asc_schedule`]'s own doc comments -- so without this,
/// `optimize_design` always takes its zero-free-tiers early return). `false` for
/// CrackOtto-Step -- see this file's module doc comment for why.
///
/// # Panics
///
/// If any pipeline step fails -- this crate's own test suite already holds every
/// step here to "must succeed" for a template or a real, well-formed design (see
/// `templates::tests::every_template_solves_and_closes`), so a failure here means a
/// real regression this training run should surface loudly rather than silently
/// skip, exactly like `crates/indicatrix/examples/pgo_train/stages.rs`'s `train_brep`.
fn train_one_design(
    name: &str,
    design: &Design,
    original_asc: Option<&str>,
    force_free_tier: bool,
    checksum: &mut f64,
) {
    let t0 = Instant::now();

    let solved = design
        .solve()
        .unwrap_or_else(|e| panic!("{name}: Design::solve failed: {e}"));
    *checksum += solved.iter().map(|s| s.mast).sum::<f64>();

    let schedule = design.to_asc_schedule_from_solved(&solved);
    let asc_text =
        indicatrix_formats::asc::to_asc_string(&schedule).expect("training design writes");
    *checksum = (asc_text.len() as f64).mul_add(1e-3, *checksum);

    let saved = save_paired(design, format!("{name}.asc"), original_asc, None, None)
        .unwrap_or_else(|e| panic!("{name}: save_paired failed: {e}"));
    *checksum = (saved.asc_text.len() as f64).mul_add(1e-3, *checksum);

    let design_text = design_to_string(design, None, &DesignExtras::default())
        .unwrap_or_else(|e| panic!("{name}: design_to_string failed: {e}"));
    *checksum = (design_text.len() as f64).mul_add(1e-3, *checksum);

    let loaded = design_from_str(&design_text)
        .unwrap_or_else(|e| panic!("{name}: design_from_str failed: {e}"));
    let mut reloaded = loaded.design;
    *checksum += reloaded.tiers.len() as f64;

    // `resolve_after_edit` for one `Edit::MoveTier` -- swap the last two tiers (a
    // real, index-renumbering move whenever there are at least two tiers). `solved`
    // (from the very first `design.solve()` above) is a valid `previous` baseline
    // for `reloaded` too: the design file round-trips every tier's
    // angle/indices/constraint losslessly, so `reloaded` solves identically to
    // `design` -- reusing it here avoids yet another full solve on a design as
    // heavy as CrackOtto-Step.
    if reloaded.tiers.len() >= 2 {
        let from = reloaded.tiers.len() - 1;
        let to = reloaded.tiers.len() - 2;
        let edit = Edit::MoveTier { from, to };
        reloaded
            .apply_edit(edit.clone())
            .unwrap_or_else(|e| panic!("{name}: MoveTier failed: {e}"));
        let resolved = resolve_after_edit(&reloaded, &solved, &edit)
            .unwrap_or_else(|e| panic!("{name}: resolve_after_edit failed: {e}"));
        *checksum += resolved.iter().map(|s| s.mast).sum::<f64>();
    }

    // `optimize_design` with a tiny budget -- see this function's own doc comment
    // for why `force_free_tier` is `false` for CrackOtto-Step, and
    // `pick_adoptable_free_tier`'s own doc comment for why not every template with
    // `force_free_tier: true` actually gets a free tier either.
    if force_free_tier && let Some(index) = pick_adoptable_free_tier(&reloaded) {
        reloaded.tiers[index].constraint = MeetConstraint::MeetExisting;
    }
    let material = GemMaterial::diamond();
    let config = OptimizeConfig {
        max_evaluations: 4,
        polish_start_step_deg: None,
        ..OptimizeConfig::default()
    };
    let outcome = optimize_design(&reloaded, &material, &config, &SearchHooks::default())
        .unwrap_or_else(|e| panic!("{name}: optimize_design failed: {e}"));
    *checksum =
        (f64::from(outcome.after_score) + outcome.evaluations as f64).mul_add(1e-3, *checksum);

    println!(
        "  cut-core: {name}: {:.2?} ({} free tier(s), {} optimize evaluation(s))",
        t0.elapsed(),
        free_tier_indices(&reloaded).len(),
        outcome.evaluations
    );
}

fn main() {
    let start = Instant::now();
    let mut checksum = 0.0f64;

    for spec in templates::TEMPLATES {
        let design = Design::new(
            PreformSpec::cylinder(spec.gear_teeth.unsigned_abs() as usize, 1.5, 1.0, 1.5),
            spec.schedule_meta(),
            spec.tiers(),
        );
        train_one_design(spec.name, &design, None, true, &mut checksum);
    }

    let crackotto_schedule =
        indicatrix_formats::asc::parse_asc(CRACKOTTO_ASC).expect("CrackOtto-Step fixture parses");
    let crackotto_design = Design::from_asc_schedule(
        PreformSpec::cylinder(crackotto_schedule.gear_teeth_abs() as usize, 1.5, 1.0, 1.5),
        &crackotto_schedule,
    );
    train_one_design(
        "CrackOtto-Step",
        &crackotto_design,
        Some(CRACKOTTO_ASC),
        false,
        &mut checksum,
    );

    println!(
        "pgo_train (indicatrix-cut-core) done in {:.1?} (checksum={checksum:.6})",
        start.elapsed()
    );
}
