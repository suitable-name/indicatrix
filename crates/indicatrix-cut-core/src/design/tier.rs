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
}
