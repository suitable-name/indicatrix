//! [`ConstraintTier`] (one authored facet tier) and [`ScheduleMeta`] (every
//! non-tier `.asc` schedule field) -- the two plain data types
//! [`super::Design`] is built from. See the parent module's doc comment for
//! why a tier's constraint, not a stored mast, is the field this crate treats
//! as authoritative.

use indicatrix::geometry::meet_solver::MeetConstraint;

/// One authored facet tier: geometry that fixes a plane's *direction*, not its
/// depth.
///
/// Carries the same angle/index-wheel-position/name fields
/// [`indicatrix_formats::asc::AscTier`] carries besides `mast` and `notes`, plus the
/// [`MeetConstraint`] that determines where its plane sits. There is
/// deliberately no `mast` field here at all -- see the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstraintTier {
    /// Signed angle from the girdle plane, in degrees -- the same `GemCAD`
    /// convention [`indicatrix_formats::asc::AscTier::angle_deg`] documents (negative is
    /// pavilion, non-negative is crown; an unsigned `0.0` inherits the previous
    /// tier's side).
    pub angle_deg: f64,
    /// Facet name(s), joined with `/` when a tier folds more than one -- exactly
    /// [`indicatrix_formats::asc::AscTier::name`]'s own convention; see
    /// [`Self::names`].
    pub name: String,
    /// Index-wheel positions this tier's facet occurs at. Empty means a single
    /// facet at azimuth 0.
    pub indices: Vec<f64>,
    /// What determines this tier's mast: meet an unspecified vertex, meet named
    /// facets, or an authored scale dimension. See the module docs.
    pub constraint: MeetConstraint,
    /// The meet instruction a real `.asc` file's `G` field actually stated for
    /// this tier at import time ([`indicatrix::geometry::meet_solver::meet_tier_inputs_from_asc`]'s
    /// classification, [`MeetConstraint::MeetExisting`] or [`MeetConstraint::MeetNamed`] only --
    /// see [`super::Design::from_asc_schedule`]'s doc comment for why import now pins
    /// every tier's `constraint` to a [`MeetConstraint::ScaleReference`]
    /// regardless of what the file said there).
    ///
    /// This is display/adoption data, never authoritative: [`super::Design::solve`]
    /// and [`super::Design::to_asc_schedule`] read `constraint` alone. It exists so the
    /// editor can show what the file *claims* this facet meets and let the user
    /// adopt it with one click (`Edit::SetConstraint`), without losing that
    /// information the moment import pins the tier's real mast in `constraint`.
    /// `None` for a tier the file gave an explicit scale-reference instruction
    /// for (nothing to adopt -- `constraint` already reflects it), and for any
    /// tier not built by [`super::Design::from_asc_schedule`] at all (a brand-new tier
    /// the user adds in the editor, or the result of applying an `Edit`).
    pub imported_meet: Option<MeetConstraint>,
    /// The raw `.asc` `G`-field text this tier's own [`indicatrix_formats::asc::AscTier::notes`]
    /// actually carried at import time, verbatim (including an empty string when the
    /// file left the tier unnoted) -- see [`super::Design::from_asc_schedule`]'s doc
    /// comment for why this exists: synthesizing a tier's exported `G` text from
    /// [`Self::constraint`] alone loses real phrasings like `"Cut to TCP"`/`"GMP"`/
    /// `"Cut to mast depth X."` the moment `constraint` gets pinned to
    /// [`MeetConstraint::ScaleReference`] on import. `super::export`'s notes-writing
    /// step reads this back verbatim for as long as `constraint` stays the pinned
    /// value import produced; once the cutter adopts a different constraint
    /// (`Edit::SetConstraint`), export falls back to synthesizing fresh text instead,
    /// since this recorded text no longer describes what the tier now does.
    ///
    /// `None` for a tier not built by [`super::Design::from_asc_schedule`] at all --
    /// the same convention [`Self::imported_meet`] follows.
    pub original_notes: Option<String>,
    /// Index-wheel positions in `indices` that are exempted from
    /// [`crate::orbit`]'s orbit-wide propagation -- see that module's docs
    /// for what that means concretely. Always a subset of `indices` (not
    /// enforced by the type; [`super::Design::detach_orbit_member`] and
    /// [`super::Design::reattach_orbit_member`] are the only supported way to grow
    /// or shrink it, both ordinary `History`-mediated edits). Empty for
    /// every tier [`super::Design::from_asc_schedule`] produces and for a
    /// brand-new tier the user adds -- detaching is a deliberate act, never
    /// something import or tier-creation infers.
    pub detached: Vec<f64>,
}

impl ConstraintTier {
    /// Every distinct name this tier is known by -- see
    /// [`indicatrix_formats::asc::AscTier::names`], which this mirrors exactly (same `/`
    /// join convention, same empty-when-unnamed rule).
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        if self.name.is_empty() {
            Vec::new()
        } else {
            self.name.split('/').collect()
        }
    }

    /// Which other tiers (as indices into the `tiers` slice `self` came from)
    /// this tier's own constraint names as a meet target -- the read-only
    /// half of "which facets does this facet meet", for a cut sheet or an
    /// inspector to show without re-deriving the resolution the solver does
    /// internally.
    ///
    /// Only [`MeetConstraint::MeetNamed`] has named targets at all:
    /// [`MeetConstraint::MeetExisting`] means "whatever vertex the solver
    /// finds", with no name to resolve, and [`MeetConstraint::ScaleReference`]
    /// means "this tier's own authored dimension", not a meet at all -- both
    /// return an empty list. A named token resolves against `tiers`' own
    /// [`Self::names`] by first exact match, then ASCII-case-insensitive
    /// match, same precedence
    /// [`indicatrix::geometry::meet_solver::MeetNameResolver`] uses for a
    /// plain (non-compound, non-girdle/culet/table-fallback) name; an
    /// unresolved token is skipped rather than erroring, since this is a
    /// display helper, not a validator. A caller that needs the solver's
    /// full fallback rules (girdle/culet/table synonyms, side-prefix and
    /// plural stripping, compound `"1-2-G1"` vertex specs) should build a
    /// [`indicatrix::geometry::meet_solver::MeetNameResolver`] directly
    /// instead -- see [`super::Design::facet_meets`], which does exactly
    /// that.
    #[must_use]
    pub fn meet_target_indices(&self, tiers: &[Self]) -> Vec<usize> {
        let MeetConstraint::MeetNamed(names) = &self.constraint else {
            return Vec::new();
        };
        names
            .iter()
            .filter_map(|name| {
                tiers
                    .iter()
                    .position(|t| t.names().contains(&name.as_str()))
                    .or_else(|| {
                        tiers
                            .iter()
                            .position(|t| t.names().iter().any(|n| n.eq_ignore_ascii_case(name)))
                    })
            })
            .collect()
    }

    /// Builds `count` tiers forming a step-cut ladder: angle stepping by
    /// `angle_step_deg` from `start_angle_deg`, all sharing `indices`, each
    /// named `"<name_prefix><n>"` (1-based). The first tier's constraint is
    /// `first_constraint` (typically a [`MeetConstraint::ScaleReference`]
    /// anchor or a [`MeetConstraint::MeetNamed`] reference into the rest of
    /// the design, since a ladder's own first rung has nothing above it to
    /// meet); every later tier meets the one immediately above it via
    /// [`MeetConstraint::MeetExisting`], the plain nested-nudge case a real
    /// step-cut schedule authors under.
    ///
    /// Returns bare [`ConstraintTier`]s, not [`crate::edit::Edit`]s -- an
    /// editor turns them into one batched sequence of
    /// `crate::edit::Edit::AddTier` and applies it through
    /// `crate::edit::History` like any other edit, same as this crate's other
    /// pure constructors (e.g. [`Self::standard_round_brilliant`]).
    #[must_use]
    pub fn step_series(
        name_prefix: &str,
        start_angle_deg: f64,
        angle_step_deg: f64,
        count: usize,
        indices: &[f64],
        first_constraint: &MeetConstraint,
    ) -> Vec<Self> {
        (0..count)
            .map(|n| {
                let angle_deg = angle_step_deg.mul_add(n as f64, start_angle_deg);
                let constraint = if n == 0 {
                    first_constraint.clone()
                } else {
                    MeetConstraint::MeetExisting
                };
                Self {
                    angle_deg,
                    name: format!("{name_prefix}{}", n + 1),
                    indices: indices.to_vec(),
                    constraint,
                    imported_meet: None,
                    original_notes: None,
                    detached: Vec::new(),
                }
            })
            .collect()
    }

    /// A single girdle-band facet tier, pinned via [`MeetConstraint::ScaleReference`]
    /// to `half_width` (the plane's own distance from the vertical axis, exactly
    /// the "Exact scale value = half-width" convention a girdle-authoring UI
    /// control should emit).
    ///
    /// Deliberately fixed at exactly `90.0` degrees:
    /// `indicatrix::geometry::meet_solver::blocks::classify_blocks` only assigns
    /// [`indicatrix::geometry::meet_solver::Block::Girdle`] when a tier's angle
    /// is at (or within `1e-6` of) 90 degrees, so a 0-degree "girdle" -- the
    /// manual's own worked-example mistake this constructor exists to make hard
    /// to repeat -- classifies as a second table instead
    /// (see [`super::Design::from_asc_schedule`]'s doc comment for the module
    /// this crate treats as authoritative on that point) and never produces a
    /// live girdle band at all.
    #[must_use]
    pub fn girdle(name: &str, half_width: f64, indices: &[f64]) -> Self {
        Self {
            angle_deg: 90.0,
            name: name.to_string(),
            indices: indices.to_vec(),
            constraint: MeetConstraint::ScaleReference(half_width),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    /// Mirrors this tier to the opposite block (crown &lt;-&gt; pavilion): same
    /// indices and constraint, angle negated (see [`Self::angle_deg`]'s sign
    /// convention), name suffixed with `name_suffix` (e.g. `"'"` for a paired
    /// "Pavilion Main'" facing "Crown Main") -- the paired-tier half of "Mirror
    /// to other block", a routine emerald/step-cut operation.
    #[must_use]
    pub fn mirrored_to_other_block(&self, name_suffix: &str) -> Self {
        Self {
            angle_deg: -self.angle_deg,
            name: format!("{}{name_suffix}", self.name),
            indices: self.indices.clone(),
            constraint: self.constraint.clone(),
            imported_meet: None,
            original_notes: None,
            detached: self.detached.clone(),
        }
    }

    /// The standard round-brilliant tier table: table, star, crown main,
    /// upper girdle, girdle, pavilion main, lower girdle, culet -- eight
    /// tiers with real angle/index/mast figures for a well-formed RBC, each
    /// pinned via [`MeetConstraint::ScaleReference`] so a design built from
    /// this table [`super::Design::solve`]s immediately without needing any
    /// other anchor. Pair with [`ScheduleMeta::standard_round_brilliant`] for
    /// the schedule metadata (96-tooth gear, 8-fold mirrored symmetry) this
    /// table's `indices` assume; re-deriving `indices` for a different gear
    /// tooth count is `crate::orbit`'s job, not this constructor's.
    ///
    /// Ported verbatim (same angles/indices/masts) from the test fixture
    /// `apps/indicatrix-cut/src/gui/solid_preview/facet_map.rs`'s
    /// `standard_round_brilliant_design`, so a "New Design -> Standard Round
    /// Brilliant" template and that test's fixture describe the same stone.
    #[must_use]
    pub fn standard_round_brilliant() -> Vec<Self> {
        const GIRDLE_INDICES: [f64; 16] = [
            0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
            90.0,
        ];
        const BREAK_INDICES: [f64; 16] = [
            95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0,
            83.0, 85.0,
        ];
        const MAIN_INDICES: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
        const STAR_INDICES: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];

        fn tier(name: &str, angle_deg: f64, indices: &[f64], mast: f64) -> ConstraintTier {
            ConstraintTier {
                angle_deg,
                name: name.to_string(),
                indices: indices.to_vec(),
                constraint: MeetConstraint::ScaleReference(mast),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            }
        }

        vec![
            tier("Table", 0.0, &[], 0.32),
            tier("Star", 15.0, &STAR_INDICES, 0.45),
            tier("Crown Main", 34.5, &MAIN_INDICES, 0.59),
            tier("Upper Girdle", 41.0, &BREAK_INDICES, 0.67),
            tier("Girdle", 90.0, &GIRDLE_INDICES, 1.0),
            tier("Pavilion Main", -41.0, &MAIN_INDICES, 0.67),
            tier("Lower Girdle", -42.5, &BREAK_INDICES, 0.68),
            tier("Culet", -0.0, &[], 0.88),
        ]
    }
}

/// Every non-tier field of a `.asc` schedule ([`indicatrix_formats::asc::AscSchedule`]'s own fields minus
/// `tiers`): index gear, symmetry, refractive index, and free-text header/
/// footnote lines.
///
/// Split out from [`indicatrix_formats::asc::AscSchedule`] itself because a [`super::Design`]'s tiers are
/// [`ConstraintTier`]s (authored constraints), not [`indicatrix_formats::asc::AscTier`]s
/// (recorded masts) -- see the module docs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScheduleMeta {
    pub gemcad_version: String,
    pub gear_teeth: i32,
    pub gear_reference_angle: f64,
    pub symmetry_order: u32,
    pub mirror: bool,
    /// The legacy `.asc` `I` line's refractive index, as last imported or
    /// exported. [`super::Design::effective_refractive_index`] is the value
    /// everything else in this crate actually scores/exports/critical-angles
    /// against (override, then a recognized material name, then this field only
    /// as the final fallback); this field is read directly only when neither of
    /// those apply.
    ///
    /// **Overwritten on an export/re-import round trip** (a known, unfixed
    /// data-loss path): `Design::to_asc_schedule[_from_solved]` writes
    /// `effective_refractive_index()` -- not this field -- as the exported `I`
    /// line, and `Design::from_asc_schedule` then reads that same freshly
    /// exported line straight back into this field on the next import. So
    /// picking a recognized material (even without an explicit override) and
    /// then saving-and-reopening through `.asc` permanently replaces whatever
    /// this field held with the material's own resolved `n_D` -- there is no way
    /// to recover the original legacy value afterward, though in practice this
    /// rarely matters since `effective_refractive_index` would already have
    /// preferred the material/override over this field anyway.
    pub refractive_index: f64,
    pub headers: Vec<String>,
    pub footnotes: Vec<String>,
}

impl ScheduleMeta {
    /// The index wheel's tooth count as an unsigned magnitude -- see
    /// [`indicatrix_formats::asc::AscSchedule::gear_teeth_abs`], which this mirrors exactly.
    #[must_use]
    pub const fn gear_teeth_abs(&self) -> u32 {
        self.gear_teeth.unsigned_abs()
    }

    /// A round-brilliant-appropriate schedule header: 96-tooth index gear,
    /// 8-fold mirrored symmetry, `GemCad`'s legacy diamond refractive index --
    /// the metadata [`ConstraintTier::standard_round_brilliant`]'s tier table
    /// assumes for its `indices`.
    #[must_use]
    pub fn standard_round_brilliant() -> Self {
        Self {
            gemcad_version: "GemCad 5.0".to_string(),
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            symmetry_order: 8,
            mirror: true,
            refractive_index: 1.54,
            headers: Vec::new(),
            footnotes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{design::Design, preform::PreformSpec};

    fn named_tier(name: &str, constraint: MeetConstraint) -> ConstraintTier {
        ConstraintTier {
            angle_deg: 0.0,
            name: name.to_string(),
            indices: Vec::new(),
            constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    /// `meet_target_indices` must resolve an exact name, an
    /// ASCII-case-insensitive name, skip an unresolved token, and return
    /// nothing for `MeetExisting`/`ScaleReference`.
    #[test]
    fn meet_target_indices_resolves_exact_and_case_insensitive_names() {
        let tiers = vec![
            named_tier("P1", MeetConstraint::MeetExisting),
            named_tier("P2", MeetConstraint::MeetExisting),
            named_tier(
                "Star",
                MeetConstraint::MeetNamed(vec![
                    "p1".to_string(),
                    "P2".to_string(),
                    "Nonexistent".to_string(),
                ]),
            ),
        ];
        assert_eq!(tiers[2].meet_target_indices(&tiers), vec![0, 1]);
        assert_eq!(tiers[0].meet_target_indices(&tiers), Vec::<usize>::new());
    }

    /// `girdle` must build an exactly-90-degree tier pinned to `half_width`,
    /// and -- the whole point of this constructor -- `classify_blocks` must
    /// actually classify it as `Block::Girdle`, unlike the manual's own
    /// 0-degree worked-example mistake, which classifies as `Block::Crown`.
    #[test]
    fn girdle_builds_a_90_degree_tier_that_classifies_as_girdle() {
        use indicatrix::geometry::meet_solver::{Block, MeetTierInput, classify_blocks};

        let indices = [0.0, 90.0, 180.0, 270.0];
        let girdle = ConstraintTier::girdle("Girdle", 1.0, &indices);
        assert_eq!(girdle.angle_deg, 90.0);
        assert_eq!(girdle.name, "Girdle");
        assert_eq!(girdle.indices, indices);
        assert_eq!(girdle.constraint, MeetConstraint::ScaleReference(1.0));

        let as_input = |tier: &ConstraintTier| MeetTierInput {
            angle_deg: tier.angle_deg,
            indices: tier.indices.clone(),
            constraint: tier.constraint.clone(),
            names: vec![tier.name.clone()],
        };
        // The manual's own worked-example mistake: the
        // same tier authored at 0 degrees instead of 90.
        let zero_degree_mistake = MeetTierInput {
            angle_deg: 0.0,
            ..as_input(&girdle)
        };
        let classified = classify_blocks(&[as_input(&girdle), zero_degree_mistake]);
        assert_eq!(classified[0], Block::Girdle);
        assert_eq!(classified[1], Block::Crown);
    }

    /// `step_series` must produce `count` tiers stepping by `angle_step_deg`,
    /// sharing `indices`, the first pinned to `first_constraint` and every
    /// later one meeting the tier above it.
    #[test]
    fn step_series_builds_a_ladder_meeting_the_tier_above() {
        let indices = [0.0, 90.0];
        let tiers = ConstraintTier::step_series(
            "Step",
            -20.0,
            -5.0,
            3,
            &indices,
            &MeetConstraint::ScaleReference(0.5),
        );
        assert_eq!(tiers.len(), 3);
        assert_eq!(tiers[0].angle_deg, -20.0);
        assert_eq!(tiers[1].angle_deg, -25.0);
        assert_eq!(tiers[2].angle_deg, -30.0);
        assert_eq!(tiers[0].name, "Step1");
        assert_eq!(tiers[2].name, "Step3");
        assert_eq!(tiers[0].constraint, MeetConstraint::ScaleReference(0.5));
        assert_eq!(tiers[1].constraint, MeetConstraint::MeetExisting);
        assert_eq!(tiers[2].constraint, MeetConstraint::MeetExisting);
        for tier in &tiers {
            assert_eq!(tier.indices, indices);
        }
    }

    /// `mirrored_to_other_block` must negate the angle and keep everything
    /// else, with the name suffix appended.
    #[test]
    fn mirrored_to_other_block_negates_angle_and_suffixes_name() {
        let tier = ConstraintTier {
            angle_deg: 34.5,
            name: "Main".to_string(),
            indices: vec![0.0, 90.0],
            constraint: MeetConstraint::ScaleReference(0.59),
            imported_meet: None,
            original_notes: None,
            detached: vec![0.0],
        };
        let mirrored = tier.mirrored_to_other_block("'");
        assert_eq!(mirrored.angle_deg, -34.5);
        assert_eq!(mirrored.name, "Main'");
        assert_eq!(mirrored.indices, tier.indices);
        assert_eq!(mirrored.constraint, tier.constraint);
        assert_eq!(mirrored.detached, tier.detached);
    }

    /// The standard round-brilliant template must actually solve and close
    /// as a real solid -- not just produce eight plausible-looking tiers.
    #[test]
    fn standard_round_brilliant_template_solves_and_closes() {
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        assert_eq!(design.tiers.len(), 8);
        let solved = design
            .solve()
            .expect("every tier is pinned via ScaleReference");
        assert_eq!(solved.len(), 8);
        assert!(design.is_closed());
    }
}
