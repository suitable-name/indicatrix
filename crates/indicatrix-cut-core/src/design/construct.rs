//! [`Design`]'s three constructors -- see the parent module's doc comment for
//! the constraints-not-masts model these build against, and
//! [`Design::from_asc_schedule`]'s own doc comment for the "pin every tier"
//! import policy this crate settled on.

use super::{ConstraintTier, Design, ScheduleMeta};
use crate::{material::MaterialSelection, preform::PreformSpec};
use indicatrix::geometry::meet_solver::{MeetConstraint, meet_tier_inputs_from_asc};
use indicatrix_formats::asc::AscSchedule;
use std::collections::BTreeMap;

/// Everything [`Design::fresh_from_spec`] needs to build a brand-new design.
///
/// The index-gear/symmetry/mirror settings a real `.asc` file would open with, the
/// starting material selection, and the preform to cut it from. Replaces
/// [`Design::fresh`]'s four positional parameters with named fields wide enough to
/// also carry a starting material; the editor's "New" dialog fills one of these
/// directly.
#[derive(Debug, Clone, PartialEq)]
pub struct FreshDesignSpec {
    pub gear_teeth: i32,
    pub symmetry_order: u32,
    pub mirror: bool,
    pub material: MaterialSelection,
    pub preform: PreformSpec,
}

impl Design {
    /// The legacy default [`ScheduleMeta::refractive_index`] a fresh design with no
    /// material selection starts with -- the same `1.54` [`Design::fresh`] always
    /// hard-coded before a real material model existed. Only read by
    /// [`Design::effective_refractive_index`] once a design has neither an RI
    /// override nor a material name that resolves.
    const LEGACY_DEFAULT_REFRACTIVE_INDEX: f64 = 1.54;

    /// Pairs an existing preform, schedule metadata and constraint-tier list, with no
    /// scale anchor or material selection yet. A caller that wants those set from the
    /// start uses a struct-update literal or applies
    /// `Edit::SetGirdleDiameterMm`/`Edit::SetMaterial` afterward through `History`.
    #[must_use]
    pub const fn new(preform: PreformSpec, meta: ScheduleMeta, tiers: Vec<ConstraintTier>) -> Self {
        Self {
            preform,
            meta,
            tiers,
            girdle_diameter_mm: None,
            preform_y_offset: 0.0,
            cheater_offsets_deg: BTreeMap::new(),
            material: MaterialSelection::none(),
        }
    }

    /// A brand-new design from a full [`FreshDesignSpec`] -- gear, symmetry, mirror
    /// and starting material all named up front ([`Design::fresh`] is kept as a thin
    /// wrapper over this for source compatibility). No tiers yet; the schedule's
    /// legacy [`ScheduleMeta::refractive_index`] starts at
    /// [`Self::LEGACY_DEFAULT_REFRACTIVE_INDEX`], only read as a fallback by
    /// [`Design::effective_refractive_index`] since a resolved material's `n_D`
    /// already wins over it.
    #[must_use]
    pub fn fresh_from_spec(spec: FreshDesignSpec) -> Self {
        Self {
            preform: spec.preform,
            meta: ScheduleMeta {
                gemcad_version: "5.0".to_string(),
                gear_teeth: spec.gear_teeth,
                symmetry_order: spec.symmetry_order,
                mirror: spec.mirror,
                refractive_index: Self::LEGACY_DEFAULT_REFRACTIVE_INDEX,
                ..ScheduleMeta::default()
            },
            tiers: Vec::new(),
            girdle_diameter_mm: None,
            preform_y_offset: 0.0,
            cheater_offsets_deg: BTreeMap::new(),
            material: spec.material,
        }
    }

    /// A brand-new design: `preform`, and an empty schedule carrying only the
    /// index-gear/symmetry/refractive-index settings a real `.asc` file would open
    /// with. No tiers yet -- see [`Design::planes`] and `fresh_design_alone_is_closed`
    /// for why that's still a fully renderable solid, not an empty viewport.
    ///
    /// A thin wrapper over [`Design::fresh_from_spec`] (`mirror` always `true`, no
    /// material selection) kept for source compatibility; call
    /// [`Design::fresh_from_spec`] directly to set either.
    #[must_use]
    pub fn fresh(
        preform: PreformSpec,
        gear_teeth: i32,
        symmetry_order: u32,
        refractive_index: f64,
    ) -> Self {
        let mut design = Self::fresh_from_spec(FreshDesignSpec {
            gear_teeth,
            symmetry_order,
            mirror: true,
            material: MaterialSelection::none(),
            preform,
        });
        design.meta.refractive_index = refractive_index;
        design
    }

    /// Builds a [`Design`] from a real, parsed `.asc` schedule by **pinning every
    /// tier's mast exactly**: each tier's [`MeetConstraint`] becomes
    /// [`MeetConstraint::ScaleReference`] at that tier's own real recorded
    /// `mast`, full stop -- regardless of what the file's `G` field said there.
    /// Re-solving a design built this way (`design.solve()`) therefore
    /// reproduces every original mast **exactly** (to float rounding, not a
    /// tolerance): opening a catalogue design in the editor must show the same
    /// stone the file records, not a solver's reconstruction of it.
    ///
    /// # Why this replaced discard-and-rederive
    ///
    /// Rejected: anchoring only one tier per crown/pavilion/girdle block and
    /// re-deriving every other tier's mast from constraints alone via
    /// `solve_meet_points`. Measured on the full 2,881-design corpus, only **312
    /// designs (10.8%)** had every meet-derived tier within 10% of its real recorded
    /// mast -- opening an ordinary catalogue design would have silently moved its
    /// geometry for roughly nine designs in ten the moment someone clicked "Solve"
    /// (see `discard_and_resolve_gate_on_real_fixtures` below).
    ///
    /// Pinning every tier makes adopting a meet-constraint a **deliberate, per-tier,
    /// undoable act** instead: [`ConstraintTier::imported_meet`] preserves the
    /// file's own stated meet instruction alongside the pinned constraint, so the
    /// editor can show what the file *claims* a facet meets and let the user switch
    /// that tier over to it with one click (`Edit::SetConstraint`) like any other
    /// edit. A tier with an explicit scale-reference instruction has nothing to
    /// adopt, so `imported_meet` is `None` there.
    ///
    /// [`ConstraintTier::original_notes`] separately preserves the file's raw `G`
    /// text verbatim (whatever it said, not just the three instructions this crate
    /// classifies), so exporting an untouched design writes back the same notes a
    /// human reads while cutting rather than a synthesized stand-in.
    #[must_use]
    pub fn from_asc_schedule(preform: PreformSpec, schedule: &AscSchedule) -> Self {
        let inputs = meet_tier_inputs_from_asc(schedule);

        let tiers = inputs
            .into_iter()
            .zip(&schedule.tiers)
            .map(|(input, original)| {
                // Anything that isn't already a stated scale reference is worth
                // keeping around for one-click adoption; a stated `ScaleReference`
                // has nothing to adopt.
                let imported_meet = match input.constraint {
                    MeetConstraint::ScaleReference(_) => None,
                    other => Some(other),
                };
                ConstraintTier {
                    angle_deg: input.angle_deg,
                    name: original.name.clone(),
                    indices: input.indices,
                    constraint: MeetConstraint::ScaleReference(original.mast),
                    imported_meet,
                    // Verbatim, even when empty -- see `ConstraintTier::original_notes`'s
                    // own doc comment for why export needs this to undo the "every tier
                    // pinned to ScaleReference" policy's flattening effect on `G` text.
                    original_notes: Some(original.notes.clone()),
                    detached: Vec::new(),
                }
            })
            .collect();

        Self {
            preform,
            meta: ScheduleMeta {
                gemcad_version: schedule.gemcad_version.clone(),
                gear_teeth: schedule.gear_teeth,
                gear_reference_angle: schedule.gear_reference_angle,
                symmetry_order: schedule.symmetry_order,
                mirror: schedule.mirror,
                refractive_index: schedule.refractive_index,
                headers: schedule.headers.clone(),
                footnotes: schedule.footnotes.clone(),
            },
            tiers,
            // `.asc` has no field for either -- see `Self::girdle_diameter_mm`'s own
            // doc comment ("This does NOT round-trip through `.asc`").
            girdle_diameter_mm: None,
            preform_y_offset: 0.0,
            cheater_offsets_deg: BTreeMap::new(),
            material: MaterialSelection::none(),
        }
    }
}
