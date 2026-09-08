//! "Retarget for material" -- proposing and applying a new set of facet angles when
//! a design's material changes.
//!
//! Pure Rust, no Slint types: [`build_proposal`] takes a `Design` and a resolved
//! target material and returns a [`RetargetProposal`] the dialog shows for review;
//! [`apply`] turns an accepted proposal into the one `Edit::RetargetAngles` the
//! caller pushes through `History`, exactly like every other edit in this crate.
//!
//! # The two algorithms
//!
//! - [`RetargetMode::Shift`] (default): every pavilion tier's angle moves by
//!   `retarget_angle_deg`, which keeps that tier's margin over the critical angle
//!   exactly fixed; every crown tier moves by [`CrownShift`]'s policy. Girdle tiers
//!   are never touched -- not even listed in [`RetargetProposal::rows`]: a tier at
//!   (or near) plus-or-minus 90 degrees from the girdle plane classifies as
//!   `Block::Girdle`, structural rather than optical.
//! - [`RetargetMode::Optimize`] seeds a clone of the design with the `Shift` angles
//!   above, then runs `optimize_design` unchanged (only the objective material
//!   differs) over whatever tiers that search already treats as free
//!   (non-`ScaleReference`). Since an anchored tier's angle can never actually move
//!   under that search, retargeting a currently-anchored tier via this mode is
//!   refused up front ([`RetargetError::AnchoredTiers`]) rather than silently
//!   leaving it at its shifted-but-unoptimized seed -- the dialog tells the user to
//!   adopt those tiers first.
//!
//! # Why `apply` also takes the `Design`
//!
//! [`RetargetProposal::rows`] carries each row's `old_angle` for display, but
//! [`apply`] re-reads the CURRENT `angle_deg` from `design` when it builds the
//! `Edit::RetargetAngles` -- trust runtime state, not a caller's claimed previous
//! value. This also means a proposal is safe to apply even if the design moved
//! slightly between building it and pressing Apply; a row naming a tier index the
//! design no longer has is dropped rather than panicking.

use indicatrix::geometry::meet_solver::{Block, MeetConstraint, classify_blocks};
use indicatrix_cut_core::{
    Design, Edit, MissingAnchor, OptimizeConfig, ResolvedMaterial, Risk, SearchHooks,
    critical_angle_deg, optimize_design, retarget_angle_deg, tier_margin_deg, windowing_risk,
};

/// The optimizer's own safety bound: no retarget proposal -- from either mode --
/// ever proposes an angle steeper than this.
const OPTIMIZER_SAFETY_BOUND_DEG: f64 = 89.5;

/// How a crown tier's angle follows the pavilion critical-angle shift.
///
/// By default (`fraction: 0.0`) the crown is left alone. A caller can move it by a
/// fraction of the same delta every pavilion tier shifts by:
/// `critical_angle_deg(n_to) - critical_angle_deg(n_from)` is one constant number,
/// not per-tier, because the margin-preserving shift formula reduces to that
/// constant added to `theta` regardless of a tier's starting angle.
///
/// Or, when `scale_by_ratio` is set, scale the raw angle by the ratio of the two
/// critical angles instead: `theta' = theta * critical_angle_deg(n_to) /
/// critical_angle_deg(n_from)`. `scale_by_ratio` wins over `fraction` when both are
/// set away from their defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrownShift {
    pub fraction: f64,
    pub scale_by_ratio: bool,
}

impl Default for CrownShift {
    fn default() -> Self {
        Self {
            fraction: 0.0,
            scale_by_ratio: false,
        }
    }
}

/// Which of the two algorithms builds the proposal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RetargetMode {
    /// The deterministic critical-angle shift -- always available, never fails to
    /// solve (it never even calls `Design::solve`).
    Shift,
    /// Seeds from the shift above, then runs `optimize_design` (unchanged) with
    /// `config` against the resolved target material.
    Optimize(OptimizeConfig),
}

/// One reviewable row of a [`RetargetProposal`] -- one per pavilion or crown tier
/// (girdle tiers are never listed, see the module doc comment).
#[derive(Debug, Clone, PartialEq)]
pub struct RetargetRow {
    /// This tier's position in `design.tiers`.
    pub tier_index: usize,
    pub block: Block,
    pub name: String,
    pub old_angle: f64,
    pub new_angle: f64,
    /// `new_angle`'s margin over the TARGET material's critical angle.
    pub margin_deg: f64,
    pub risk: Risk,
}

/// What [`build_proposal`] returns: one row per retargeted tier, the resolved
/// target material the rows were computed against, and any caller-facing notes.
#[derive(Debug, Clone, PartialEq)]
pub struct RetargetProposal {
    pub rows: Vec<RetargetRow>,
    pub target: ResolvedMaterial,
    pub notes: Vec<String>,
}

/// Why [`build_proposal`] could not build a proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetargetError {
    /// `RetargetMode::Optimize` needs a real solved baseline before it can search
    /// at all -- propagated from `optimize_design`/`Design::solve` verbatim.
    Solve(MissingAnchor),
    /// `RetargetMode::Optimize` was asked to retarget one or more tiers currently
    /// `ScaleReference` (so `optimize_design` can never move them). `(tier_index,
    /// name)` per anchored tier, in schedule order.
    AnchoredTiers(Vec<(usize, String)>),
}

impl std::fmt::Display for RetargetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Solve(err) => write!(f, "design does not solve: {err}"),
            Self::AnchoredTiers(tiers) => {
                write!(f, "adopt these tiers' meet constraint before optimizing: ")?;
                for (i, (index, name)) in tiers.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "#{index} \"{name}\"")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for RetargetError {}

/// `true` iff `constraint` is a `ScaleReference` -- the predicate both
/// [`build_proposal`]'s anchored-tier check and `optimize::free_tier_indices` key off.
const fn is_scale_reference(constraint: &MeetConstraint) -> bool {
    matches!(constraint, MeetConstraint::ScaleReference(_))
}

/// Clamps `angle_deg` to the optimizer's safety bound, preserving sign.
fn clamp_to_safety_bound(angle_deg: f64) -> f64 {
    angle_deg.clamp(-OPTIMIZER_SAFETY_BOUND_DEG, OPTIMIZER_SAFETY_BOUND_DEG)
}

/// The critical-angle shift for one pavilion tier -- `retarget_angle_deg` clamped
/// to the safety bound. A tier whose shifted angle would sit below the new
/// critical angle isn't moved back above it: its row simply carries a negative
/// `margin_deg` and `Risk::Windows`.
fn shifted_pavilion_angle(old_angle: f64, n_from: f64, n_to: f64) -> f64 {
    clamp_to_safety_bound(retarget_angle_deg(old_angle, n_from, n_to))
}

/// The crown-tier counterpart to [`shifted_pavilion_angle`] -- see [`CrownShift`]
/// for both policies.
fn shifted_crown_angle(old_angle: f64, n_from: f64, n_to: f64, crown: CrownShift) -> f64 {
    let new_angle = if crown.scale_by_ratio {
        old_angle * critical_angle_deg(n_to) / critical_angle_deg(n_from)
    } else {
        crown.fraction.mul_add(
            critical_angle_deg(n_to) - critical_angle_deg(n_from),
            old_angle,
        )
    };
    clamp_to_safety_bound(new_angle)
}

/// The shifted angle for tier `index` under its own [`Block`] -- girdle tiers are
/// never called with this, but the arm exists so this stays a total function.
fn shifted_angle(
    design: &Design,
    index: usize,
    block: Block,
    n_from: f64,
    n_to: f64,
    crown: CrownShift,
) -> f64 {
    let old_angle = design.tiers[index].angle_deg;
    match block {
        Block::Pavilion => shifted_pavilion_angle(old_angle, n_from, n_to),
        Block::Crown => shifted_crown_angle(old_angle, n_from, n_to, crown),
        Block::Girdle => old_angle,
    }
}

/// A one-line summary of the material move, plus (for `Optimize`) a note that
/// the search only ever touches free tiers.
fn build_notes(mode: RetargetMode, n_from: f64, n_to: f64) -> Vec<String> {
    let delta = critical_angle_deg(n_to) - critical_angle_deg(n_from);
    let mut notes = vec![format!(
        "Retargeting from n_D {n_from:.4} to n_D {n_to:.4}: critical angle moves by {delta:+.3} deg."
    )];
    if matches!(mode, RetargetMode::Optimize(_)) {
        notes.push(
            "Optimize mode seeds from the critical-angle shift, then searches free tiers only; anchored tiers keep their seeded angle."
                .to_string(),
        );
    }
    notes
}

/// Builds a [`RetargetRow`] for tier `index`, already-shifted to `new_angle`.
fn row_for(design: &Design, index: usize, block: Block, new_angle: f64, n_to: f64) -> RetargetRow {
    RetargetRow {
        tier_index: index,
        block,
        name: design.tiers[index].name.clone(),
        old_angle: design.tiers[index].angle_deg,
        new_angle,
        margin_deg: tier_margin_deg(new_angle, n_to),
        risk: windowing_risk(new_angle, n_to),
    }
}

/// Builds a retarget proposal for `design` against `target`, per `mode` -- see the
/// module doc comment for both algorithms.
///
/// `design`'s `effective_refractive_index` is the "from" index; `target.n_d` is
/// the "to" index.
///
/// # Errors
///
/// - [`RetargetError::AnchoredTiers`] (`Optimize` only): one or more pavilion/crown
///   tiers this proposal would otherwise retarget are still `ScaleReference`-pinned.
///   Adopt them first, then call this again.
/// - [`RetargetError::Solve`] (`Optimize` only): `design` itself does not solve.
///
/// `RetargetMode::Shift` never fails: pure angle arithmetic, never calls `Design::solve`.
#[must_use = "a proposal must be shown to the user before it is applied"]
pub fn build_proposal(
    design: &Design,
    target: &ResolvedMaterial,
    crown: CrownShift,
    mode: RetargetMode,
) -> Result<RetargetProposal, RetargetError> {
    let n_from = design.effective_refractive_index();
    let n_to = target.n_d;

    let inputs = design.meet_tier_inputs();
    let blocks = classify_blocks(&inputs);

    // Every pavilion/crown tier, in schedule order. Girdle tiers are never part
    // of this set -- not filtered out later, never considered in the first place.
    let scope: Vec<usize> = (0..design.tiers.len())
        .filter(|&i| blocks[i] != Block::Girdle)
        .collect();

    let final_angles = match mode {
        RetargetMode::Shift => scope
            .iter()
            .map(|&i| shifted_angle(design, i, blocks[i], n_from, n_to, crown))
            .collect(),
        RetargetMode::Optimize(config) => {
            build_optimized_angles(design, target, &scope, crown, &config)?
        }
    };

    let rows = scope
        .iter()
        .zip(&final_angles)
        .map(|(&index, &new_angle)| row_for(design, index, blocks[index], new_angle, n_to))
        .collect();

    Ok(RetargetProposal {
        rows,
        target: target.clone(),
        notes: build_notes(mode, n_from, n_to),
    })
}

/// [`build_proposal`]'s `RetargetMode::Optimize` half, split out to keep
/// `build_proposal` itself short. Seeds a clone of `design` with the shift angles
/// for every tier in `scope`, runs `optimize_design` unchanged against
/// `target.gem`, then reads back the final angle for each `scope` tier (the seeded
/// angle, overridden by `AngleChange::to_deg` for whichever tiers actually moved).
///
/// # Errors
///
/// See [`build_proposal`]'s `# Errors` section.
fn build_optimized_angles(
    design: &Design,
    target: &ResolvedMaterial,
    scope: &[usize],
    crown: CrownShift,
    config: &OptimizeConfig,
) -> Result<Vec<f64>, RetargetError> {
    let anchored: Vec<(usize, String)> = scope
        .iter()
        .filter(|&&i| is_scale_reference(&design.tiers[i].constraint))
        .map(|&i| (i, design.tiers[i].name.clone()))
        .collect();
    if !anchored.is_empty() {
        return Err(RetargetError::AnchoredTiers(anchored));
    }

    let n_from = design.effective_refractive_index();
    let n_to = target.n_d;
    let blocks = classify_blocks(&design.meet_tier_inputs());

    let mut seeded = design.clone();
    for &i in scope {
        seeded.tiers[i].angle_deg = shifted_angle(design, i, blocks[i], n_from, n_to, crown);
    }

    let outcome = optimize_design(&seeded, &target.gem, config, &SearchHooks::default())
        .map_err(RetargetError::Solve)?;

    let mut final_angles: Vec<f64> = scope.iter().map(|&i| seeded.tiers[i].angle_deg).collect();
    for change in &outcome.changes {
        if let Some(slot) = scope.iter().position(|&i| i == change.index) {
            final_angles[slot] = change.to_deg;
        }
    }
    Ok(final_angles)
}

/// Turns an accepted [`RetargetProposal`] into the one `Edit::RetargetAngles` the
/// caller pushes through `History::apply`.
///
/// See the module doc comment for why `design` (the CURRENT design, not
/// necessarily the one `build_proposal` was called against) is read here rather
/// than trusting each row's `old_angle`.
#[must_use]
pub fn apply(design: &Design, proposal: &RetargetProposal) -> Edit {
    let changes = proposal
        .rows
        .iter()
        .filter(|row| row.tier_index < design.tiers.len())
        .map(|row| {
            let current = design.tiers[row.tier_index].angle_deg;
            (row.tier_index, current, row.new_angle)
        })
        .collect();
    Edit::RetargetAngles { changes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::{
        BuiltinMaterials, ConstraintTier, History, MaterialSelection, PreformSpec, ScheduleMeta,
    };

    /// "RBC-445" (PC 13.156) -- a small, genuinely meet-derived design (not a
    /// synthetic worst case), the right fixture for exercising real editor behavior.
    const RBC_445: &str = "GemCad 5.0\ng 96 0.0\ny 3 y\nI 1.54\n\
         H PC 13.156  RBC-445\n\
         H Richard B Conley, Facets, Oct 2013 p10\n\
         a -50.400000 0.61624001 92 n 1 68 60 36 28 4 G TCP\n\
         a -48.900000 0.63552822 88 n 2 72 56 40 24 8 G TCP\n\
         a -90.000000 0.91252100 92 n 3 68 60 36 28 4\n\
         a -90.000000 0.96225045 88 n 4 72 56 40 24 8\n\
         a -47.200000 0.59908304 95 n 5 65 63 33 31 1\n\
         a -43.000000 0.64485570 86 n 6 74 54 42 22 10 G PCP\n\
         a -47.266965 0.63949242 87 n 7 73 55 41 23 9\n\
         a 31.000000 0.62509296 4 n A 28 36 60 68 92\n\
         a 29.000000 0.62477630 8 n B 24 40 56 72 88\n\
         a 28.100000 0.60078994 2 n C 30 34 62 66 94\n\
         a 20.940747 0.56272062 14 n D 18 46 50 78 82\n\
         a 0.000000 0.40674031 96 n E\n";

    /// Builds a real `Design` with genuine meet-derived structure from a raw `.asc`
    /// text: every tier keeps its file's real `MeetConstraint`, except that each
    /// crown/pavilion/girdle `Block` present gets exactly one bootstrapped
    /// `ScaleReference` (that block's first tier's real recorded mast) when the
    /// file stated no explicit anchor of its own.
    fn design_with_real_meet_structure(text: &str) -> Design {
        let schedule = indicatrix_formats::asc::parse_asc(text).expect("fixture must parse");
        let mut inputs = indicatrix::geometry::meet_solver::meet_tier_inputs_from_asc(&schedule);
        let blocks = classify_blocks(&inputs);
        for block in [Block::Crown, Block::Pavilion, Block::Girdle] {
            let anchored = inputs
                .iter()
                .zip(&blocks)
                .any(|(t, &b)| b == block && is_scale_reference(&t.constraint));
            if anchored {
                continue;
            }
            if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
                inputs[i].constraint = MeetConstraint::ScaleReference(schedule.tiers[i].mast);
            }
        }
        let tiers = inputs
            .into_iter()
            .zip(&schedule.tiers)
            .map(|(input, original)| ConstraintTier {
                angle_deg: input.angle_deg,
                name: original.name.clone(),
                indices: input.indices,
                constraint: input.constraint,
                imported_meet: None,
                detached: Vec::new(),
            })
            .collect();
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta {
                gemcad_version: schedule.gemcad_version.clone(),
                gear_teeth: schedule.gear_teeth,
                gear_reference_angle: schedule.gear_reference_angle,
                symmetry_order: schedule.symmetry_order,
                mirror: schedule.mirror,
                refractive_index: schedule.refractive_index,
                headers: schedule.headers.clone(),
                footnotes: schedule.footnotes,
            },
            tiers,
        )
    }

    fn rbc_445() -> Design {
        // The raw fixture text only carries the legacy `I 1.54` schedule field;
        // giving the design a real Diamond `MaterialSelection` makes
        // `effective_refractive_index` return Diamond's own resolved n_D (~2.417)
        // instead of the unrelated legacy 1.54 figure.
        let mut design = design_with_real_meet_structure(RBC_445);
        design.material = MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        design
    }

    fn quartz() -> ResolvedMaterial {
        MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        }
        .resolve(&BuiltinMaterials)
    }

    fn diamond_n_d() -> f64 {
        indicatrix_cut_core::built_in_refractive_index("Diamond").unwrap()
    }

    /// The RBC-445 tiers `classify_blocks` actually reports as `Block::Pavilion` --
    /// notably NOT the two tiers authored at exactly -90 degrees (indices 2 and
    /// 3): a facet at plus or minus 90 degrees from the girdle plane classifies as
    /// `Block::Girdle`, not as an extreme pavilion angle.
    fn true_pavilion_indices(design: &Design) -> Vec<usize> {
        let blocks = classify_blocks(&design.meet_tier_inputs());
        (0..design.tiers.len())
            .filter(|&i| blocks[i] == Block::Pavilion)
            .collect()
    }

    // --- RBC-445 diamond -> quartz lists every pavilion tier with the expected shift ---

    #[test]
    fn shift_mode_lists_every_true_pavilion_tier_with_the_expected_shift() {
        let design = rbc_445();
        let pavilion_indices = true_pavilion_indices(&design);
        assert_eq!(
            pavilion_indices,
            vec![0, 1, 4, 5, 6],
            "RBC-445's own -90 degree tiers (indices 2, 3) must classify as Girdle, not Pavilion"
        );

        let target = quartz();
        let n_from = design.effective_refractive_index();
        assert!((n_from - diamond_n_d()).abs() < 1e-9);
        let n_to = target.n_d;

        let proposal = build_proposal(&design, &target, CrownShift::default(), RetargetMode::Shift)
            .expect("Shift mode never fails");

        for &index in &pavilion_indices {
            let row = proposal
                .rows
                .iter()
                .find(|r| r.tier_index == index)
                .unwrap_or_else(|| panic!("tier {index} must be in the proposal"));
            assert_eq!(row.block, Block::Pavilion);
            let old_angle = design.tiers[index].angle_deg;
            let expected = retarget_angle_deg(old_angle, n_from, n_to).clamp(-89.5, 89.5);
            assert!(
                (row.new_angle - expected).abs() < 1e-9,
                "tier {index}: old={old_angle} new={} expected={expected}",
                row.new_angle
            );
            // The margin-preserving formula must actually hold for every
            // tier that was not clamped.
            if expected.abs() < 89.5 - 1e-9 {
                let margin_before = tier_margin_deg(old_angle, n_from);
                let margin_after = tier_margin_deg(row.new_angle, n_to);
                assert!(
                    (margin_after - margin_before).abs() < 1e-9,
                    "tier {index}: margin drifted, before={margin_before} after={margin_after}"
                );
            }
        }

        // Girdle tiers (the two -90 degree ones) are never listed at all.
        assert!(
            !proposal
                .rows
                .iter()
                .any(|r| r.tier_index == 2 || r.tier_index == 3)
        );

        // Default crown fraction is 0: every crown tier's angle is unchanged.
        for row in proposal.rows.iter().filter(|r| r.block == Block::Crown) {
            assert!(
                (row.new_angle - row.old_angle).abs() < 1e-12,
                "tier {}: crown must stay put with the default CrownShift",
                row.tier_index
            );
        }
    }

    #[test]
    fn shift_mode_crown_fraction_moves_crown_by_the_same_constant_delta() {
        let design = rbc_445();
        let target = quartz();
        let n_from = design.effective_refractive_index();
        let n_to = target.n_d;
        let delta = critical_angle_deg(n_to) - critical_angle_deg(n_from);

        let crown = CrownShift {
            fraction: 0.5,
            scale_by_ratio: false,
        };
        let proposal = build_proposal(&design, &target, crown, RetargetMode::Shift).unwrap();

        for row in proposal.rows.iter().filter(|r| r.block == Block::Crown) {
            let expected = 0.5f64.mul_add(delta, row.old_angle).clamp(-89.5, 89.5);
            assert!(
                (row.new_angle - expected).abs() < 1e-9,
                "tier {}",
                row.tier_index
            );
        }
    }

    #[test]
    fn shift_mode_scale_by_ratio_scales_the_raw_crown_angle() {
        let design = rbc_445();
        let target = quartz();
        let n_from = design.effective_refractive_index();
        let n_to = target.n_d;
        let ratio = critical_angle_deg(n_to) / critical_angle_deg(n_from);

        let crown = CrownShift {
            fraction: 0.0,
            scale_by_ratio: true,
        };
        let proposal = build_proposal(&design, &target, crown, RetargetMode::Shift).unwrap();

        for row in proposal.rows.iter().filter(|r| r.block == Block::Crown) {
            let expected = (row.old_angle * ratio).clamp(-89.5, 89.5);
            assert!(
                (row.new_angle - expected).abs() < 1e-9,
                "tier {}",
                row.tier_index
            );
        }
    }

    // --- Apply then undo leaves the design byte-identical ---

    #[test]
    fn apply_then_undo_leaves_the_design_byte_identical() {
        let mut design = rbc_445();
        let original = design.clone();
        let target = quartz();

        let proposal =
            build_proposal(&design, &target, CrownShift::default(), RetargetMode::Shift).unwrap();
        assert_ne!(proposal.rows, Vec::new());

        let edit = apply(&design, &proposal);
        let mut history = History::new();
        history
            .apply(&mut design, edit)
            .expect("retarget must apply");
        assert_ne!(
            design, original,
            "apply must have actually changed something"
        );

        assert!(history.undo(&mut design).unwrap());
        assert_eq!(
            design, original,
            "undo must restore the design byte-identically"
        );
    }

    // --- Risk badges at boundaries ---

    #[test]
    fn risk_badges_match_the_windowing_risk_boundaries() {
        // Same index in and out (a no-op shift), so each tier's angle IS its own
        // margin over the critical angle, chosen exactly at each boundary.
        let n = 1.5;
        let crit = critical_angle_deg(n);
        let cases = [
            (crit - 1.0, Risk::Windows),
            (crit + 0.0, Risk::Marginal),
            (crit + 1.999, Risk::Marginal),
            (crit + 2.0, Risk::Safe),
        ];

        let tiers = cases
            .iter()
            .enumerate()
            .map(|(i, &(theta, _))| ConstraintTier {
                angle_deg: -theta,
                name: format!("P{i}"),
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(1.0),
                imported_meet: None,
                detached: Vec::new(),
            })
            .collect();
        let design = Design {
            material: MaterialSelection {
                name: None,
                specific_gravity_override: None,
                refractive_index_override: Some(n),
            },
            ..Design::new(
                PreformSpec::block(2.0, 1.0, 2.0),
                ScheduleMeta::default(),
                tiers,
            )
        };
        let target = ResolvedMaterial {
            gem: indicatrix::optics::materials::GemMaterial::diamond(),
            n_d: n,
            critical_angle_deg: crit,
        };

        let proposal =
            build_proposal(&design, &target, CrownShift::default(), RetargetMode::Shift).unwrap();
        for (row, &(_, expected_risk)) in proposal.rows.iter().zip(&cases) {
            assert_eq!(
                row.risk, expected_risk,
                "tier {}: margin {}",
                row.tier_index, row.margin_deg
            );
        }
    }

    // --- The anchored-tier refusal ---

    #[test]
    fn optimize_mode_refuses_anchored_tiers_and_lists_them() {
        let design = rbc_445();
        let target = quartz();
        let config = OptimizeConfig {
            max_evaluations: 4,
            ..OptimizeConfig::default()
        };

        let err = build_proposal(
            &design,
            &target,
            CrownShift::default(),
            RetargetMode::Optimize(config),
        )
        .expect_err("RBC-445's bootstrapped anchors must trigger a refusal");

        match err {
            RetargetError::AnchoredTiers(tiers) => {
                let indices: Vec<usize> = tiers.iter().map(|&(i, _)| i).collect();
                // Index 0 (pavilion "1") and index 7 (crown "A") are each the first
                // tier of their own block -- what the bootstrap pins as that
                // block's sole `ScaleReference` anchor.
                assert_eq!(indices, vec![0, 7]);
            }
            RetargetError::Solve(e) => panic!("expected AnchoredTiers, got Solve({e})"),
        }
    }

    #[test]
    fn optimize_mode_succeeds_when_nothing_in_scope_is_anchored() {
        // A design with only a Girdle tier: `scope` is empty, so the anchored
        // check passes vacuously and `optimize_design` runs against an empty free
        // set rather than refusing.
        let tiers = vec![ConstraintTier {
            angle_deg: 90.0,
            name: "Girdle".to_string(),
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(1.0),
            imported_meet: None,
            detached: Vec::new(),
        }];
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::default(),
            tiers,
        );
        let blocks = classify_blocks(&design.meet_tier_inputs());
        assert_eq!(blocks, vec![Block::Girdle]);

        let target = quartz();
        let config = OptimizeConfig {
            max_evaluations: 4,
            ..OptimizeConfig::default()
        };
        let proposal = build_proposal(
            &design,
            &target,
            CrownShift::default(),
            RetargetMode::Optimize(config),
        )
        .expect("no anchored tiers are in scope, so this must succeed");
        assert_eq!(proposal.rows, Vec::new());
    }

    // --- Golden `.asc` export for a retargeted design ---

    #[test]
    fn golden_asc_export_reflects_the_retargeted_pavilion_angles() {
        let mut design = rbc_445();
        let target = quartz();
        let n_from = design.effective_refractive_index();
        let n_to = target.n_d;

        let proposal =
            build_proposal(&design, &target, CrownShift::default(), RetargetMode::Shift).unwrap();
        let edit = apply(&design, &proposal);
        let mut history = History::new();
        history
            .apply(&mut design, edit)
            .expect("retarget must apply");

        let schedule = design
            .to_asc_schedule()
            .expect("retargeted design must still solve");
        let text = indicatrix_formats::asc::to_asc_string(&schedule);

        for &index in &true_pavilion_indices(&design) {
            let expected = retarget_angle_deg(
                // The ORIGINAL angle: read it back from the proposal row, since
                // `design` has already been mutated in place.
                proposal
                    .rows
                    .iter()
                    .find(|r| r.tier_index == index)
                    .unwrap()
                    .old_angle,
                n_from,
                n_to,
            )
            .clamp(-89.5, 89.5);
            assert!(
                (schedule.tiers[index].angle_deg - expected).abs() < 1e-6,
                "tier {index}: exported angle {} != expected {expected}",
                schedule.tiers[index].angle_deg
            );
        }
        // A real, non-empty `.asc` schedule carrying the retargeted design's header.
        assert!(text.contains("RBC-445"));
    }
}
