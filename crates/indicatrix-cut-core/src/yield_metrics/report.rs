//! [`YieldReport`] and [`Design::yield_report`], the one entry point that
//! ties [`super::scale`]'s figures and [`super::fit::exceeds_preform`]
//! together against a design's current, already-solved state.

use super::{
    fit::{PreformFit, exceeds_preform},
    scale::{carat_weight, mm_per_unit, volume_mm3, volumetric_yield},
};
use crate::{design::Design, material::MaterialLookup};
use indicatrix::geometry::{meet_solver::SolvedTier, stone_metrics::measure_solid};

/// Every figure for one already-[`Design::solve`]'d design, bundled for a
/// caller (the editor UI) that wants them all at once -- see [`Design::yield_report`].
#[derive(Debug, Clone, PartialEq)]
pub struct YieldReport {
    /// [`super::scale::volumetric_yield`] of finished volume over preform volume.
    /// `None` only when the design isn't currently a closed solid, or the preform
    /// itself fails to measure (never `None` because of a missing mm scale or
    /// material -- see the parent module's doc comment).
    pub volumetric_yield: Option<f64>,
    /// [`super::scale::mm_per_unit`]'s scale factor, when
    /// [`Design::girdle_diameter_mm`] is set and the design measures. `None`
    /// otherwise -- and whenever this is `None`, `finished_volume_mm3`/
    /// `carat_weight` below are too (both depend on it).
    ///
    /// The denominator is `f.width_axis` (below), the FINISHED SOLID's own
    /// measured width -- not, despite the doc a caller might expect from
    /// "girdle width", the width at the girdle band specifically. When the
    /// authored facets don't reach all the way out to the preform's own side
    /// walls (a stone left smaller than its rough, e.g. only a table cut so
    /// far), `width_axis` is still the PREFORM's width, because that surviving
    /// wall is part of the finished-solid measurement too (see
    /// `gate_1_yield_and_carat_weight_match_a_hand_calculation`'s own worked
    /// example in `yield_metrics/tests.rs`). `mm_per_unit` -- and therefore
    /// `finished_volume_mm3`/`carat_weight` -- is anchored to whichever of the
    /// two is currently wider, understating carat weight by
    /// `(actual_girdle_width / preform_width)^3` for a stone still inside its
    /// own block's walls. This is the actual binding, not a bug to fix here --
    /// see [`Design::girdle_diameter_mm`]'s own doc comment.
    pub mm_per_unit: Option<f64>,
    /// The finished solid's own volume, converted to real mm^3 via `mm_per_unit`.
    pub finished_volume_mm3: Option<f64>,
    /// [`crate::material::MaterialSelection::effective_specific_gravity`] for
    /// `design.material` -- the SG actually used below, whether from a preset or a
    /// user override (see that method's own doc comment).
    pub specific_gravity_used: Option<f64>,
    /// [`super::scale::carat_weight`] of `finished_volume_mm3` at
    /// `specific_gravity_used`. `None` whenever either input is `None`.
    pub carat_weight: Option<f64>,
    /// [`exceeds_preform`]'s finding, when the design's own facet planes (preform
    /// aside) form a real solid that is bigger than the stated rough. `None` means
    /// "no fit problem detected" -- either it genuinely fits, or (the common case)
    /// the facets alone don't close into a solid at all, see that function's own doc
    /// comment.
    pub preform_fit: Option<PreformFit>,
}

impl Design {
    /// Every figure (volumetric yield, mm-anchored carat weight, and the
    /// preform-fit check) for this design's current state, computed from `solved` --
    /// an already-[`Self::solve`]'d (or [`Self::resolve_dirty`]'d) mast list -- rather
    /// than forcing a second solve. Same reasoning, and the same alignment contract,
    /// as [`crate::manufacturability::check_manufacturability`]'s own `solved`
    /// parameter: a real meet-derived design costs seconds to solve, and an editor
    /// that already has a `Vec<SolvedTier>` from its own "Solve" action should never
    /// pay that cost twice just to show a yield figure.
    ///
    /// Built-ins only: `specific_gravity_used` comes from
    /// [`crate::material::MaterialSelection::effective_specific_gravity`], which
    /// cannot see a CUSTOM catalogue material's own recorded SG. See
    /// [`Self::yield_report_with`] for a catalogue-aware version -- the same
    /// "built-ins-only vs. catalogue-aware" split as [`Self::effective_refractive_index`]/
    /// [`Self::effective_refractive_index_with`].
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::planes_from_solved`]: `solved` must have
    /// one entry per tier `self` currently has, in the same order.
    #[must_use]
    pub fn yield_report(&self, solved: &[SolvedTier]) -> YieldReport {
        let finished = measure_solid(&self.planes_from_solved(solved));
        let preform = measure_solid(&self.preform.planes());

        let vol_yield = match (&finished, &preform) {
            (Some(f), Some(p)) => volumetric_yield(f.volume, p.volume),
            _ => None,
        };

        let scale = match (self.girdle_diameter_mm, &finished) {
            (Some(mm), Some(f)) => mm_per_unit(mm, f.width_axis),
            _ => None,
        };

        let finished_volume_mm3 = match (&finished, scale) {
            (Some(f), Some(scale)) => Some(volume_mm3(f.volume, scale)),
            _ => None,
        };

        let specific_gravity_used = self.material.effective_specific_gravity();

        let carat = match (finished_volume_mm3, specific_gravity_used) {
            (Some(v), Some(sg)) => Some(carat_weight(v, sg)),
            _ => None,
        };

        YieldReport {
            volumetric_yield: vol_yield,
            mm_per_unit: scale,
            finished_volume_mm3,
            specific_gravity_used,
            carat_weight: carat,
            preform_fit: exceeds_preform(self, solved),
        }
    }

    /// Like [`Self::yield_report`], but resolves `self.material`'s specific gravity
    /// through `catalogue` (see [`crate::material::MaterialLookup::specific_gravity`])
    /// instead of only this crate's own built-in table, so a CUSTOM catalogue
    /// material's authored SG reaches the carat-weight estimate too. The
    /// per-design override still wins over everything, exactly as in
    /// [`Self::yield_report`]'s own built-ins-only path -- every other figure
    /// (volumetric yield, mm scale, preform fit) is identical between the two,
    /// since none of them depend on material data at all.
    ///
    /// # Panics
    ///
    /// Same alignment contract as [`Self::yield_report`]/[`Self::planes_from_solved`].
    #[must_use]
    pub fn yield_report_with(
        &self,
        solved: &[SolvedTier],
        catalogue: &dyn MaterialLookup,
    ) -> YieldReport {
        let finished = measure_solid(&self.planes_from_solved(solved));
        let preform = measure_solid(&self.preform.planes());

        let vol_yield = match (&finished, &preform) {
            (Some(f), Some(p)) => volumetric_yield(f.volume, p.volume),
            _ => None,
        };

        let scale = match (self.girdle_diameter_mm, &finished) {
            (Some(mm), Some(f)) => mm_per_unit(mm, f.width_axis),
            _ => None,
        };

        let finished_volume_mm3 = match (&finished, scale) {
            (Some(f), Some(scale)) => Some(volume_mm3(f.volume, scale)),
            _ => None,
        };

        let specific_gravity_used = self.material.effective_specific_gravity_with(catalogue);

        let carat = match (finished_volume_mm3, specific_gravity_used) {
            (Some(v), Some(sg)) => Some(carat_weight(v, sg)),
            _ => None,
        };

        YieldReport {
            volumetric_yield: vol_yield,
            mm_per_unit: scale,
            finished_volume_mm3,
            specific_gravity_used,
            carat_weight: carat,
            preform_fit: exceeds_preform(self, solved),
        }
    }
}

#[cfg(test)]
mod yield_report_with_tests {
    use super::*;
    use crate::{
        design::ConstraintTier,
        material::{BuiltinMaterials, MaterialSelection},
        preform::PreformSpec,
    };
    use indicatrix::{geometry::meet_solver::MeetConstraint, optics::materials::GemMaterial};

    struct CustomOnlyLookup;
    impl MaterialLookup for CustomOnlyLookup {
        fn lookup(&self, name: &str) -> Option<GemMaterial> {
            GemMaterial::by_name(name)
        }

        fn specific_gravity(&self, name: &str) -> Option<f64> {
            (name == "My Garnet").then_some(3.90)
        }
    }

    /// A design that closes to a real, measurable solid -- the same minimal
    /// block-preform-plus-one-anchored-tier shape as `yield_metrics::tests`' own
    /// `box_design` (not reusable directly: that helper is private to that sibling
    /// test module), just enough geometry for `carat_weight`/`specific_gravity_used`
    /// to be `Some` rather than `None` so the two methods under test can actually be
    /// told apart.
    fn solvable_design(material: MaterialSelection) -> Design {
        let mut design = Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62);
        design.tiers.push(ConstraintTier {
            angle_deg: 0.0,
            name: "T".to_string(),
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });
        design.girdle_diameter_mm = Some(8.0);
        design.material = material;
        design
    }

    /// [`Design::yield_report_with`] must match the built-ins-only
    /// [`Design::yield_report`] when the catalogue is [`BuiltinMaterials`] -- the
    /// same equivalence [`crate::material`]'s own
    /// `effective_specific_gravity_with_matches_built_ins_only_path` test pins for
    /// the method this wraps.
    #[test]
    fn yield_report_with_matches_yield_report_against_builtin_materials() {
        let design = solvable_design(MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        });
        let solved = design.solve().expect("single anchored tier must solve");
        let plain = design.yield_report(&solved);
        let with_builtins = design.yield_report_with(&solved, &BuiltinMaterials);
        assert_eq!(plain, with_builtins);
    }

    /// The end-to-end round trip this covers: a design selecting a
    /// CUSTOM material by name (one [`BuiltinMaterials`] has never heard of) gets an
    /// empty carat estimate from [`Design::yield_report`], but a real one from
    /// [`Design::yield_report_with`] once a catalogue that knows that material's SG
    /// is supplied -- exactly `EditorMaterialLookup`'s own shape once it is opted
    /// into `RenderContext::custom_specific_gravity` (`gui::editor::material_lookup`,
    /// not reachable from this crate, so mirrored here by a minimal in-test
    /// `MaterialLookup`).
    #[test]
    fn yield_report_with_resolves_a_custom_materials_sg_that_yield_report_cannot() {
        let design = solvable_design(MaterialSelection {
            name: Some("My Garnet".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        });
        let solved = design.solve().expect("single anchored tier must solve");

        let plain = design.yield_report(&solved);
        assert_eq!(
            plain.specific_gravity_used, None,
            "built-ins-only path: no 'Garnet' preset exists, see crate::material's own doc comment"
        );
        assert_eq!(plain.carat_weight, None);

        let with_catalogue = design.yield_report_with(&solved, &CustomOnlyLookup);
        assert_eq!(with_catalogue.specific_gravity_used, Some(3.90));
        assert!(with_catalogue.carat_weight.is_some());
        // Every other figure is unaffected by which specific-gravity path was taken.
        assert_eq!(with_catalogue.volumetric_yield, plain.volumetric_yield);
        assert_eq!(with_catalogue.mm_per_unit, plain.mm_per_unit);
        assert_eq!(
            with_catalogue.finished_volume_mm3,
            plain.finished_volume_mm3
        );
        assert_eq!(with_catalogue.preform_fit, plain.preform_fit);
    }

    /// A per-design override still wins over the catalogue, exactly as it wins over
    /// the built-ins-only table in [`Design::yield_report`].
    #[test]
    fn yield_report_with_override_wins_over_the_catalogue() {
        let design = solvable_design(MaterialSelection {
            name: Some("My Garnet".to_string()),
            specific_gravity_override: Some(4.10),
            refractive_index_override: None,
        });
        let solved = design.solve().expect("single anchored tier must solve");
        let report = design.yield_report_with(&solved, &CustomOnlyLookup);
        assert_eq!(report.specific_gravity_used, Some(4.10));
    }
}
