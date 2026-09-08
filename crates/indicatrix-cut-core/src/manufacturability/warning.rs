//! [`ManufacturabilityWarning`] itself -- the one type every check reports
//! through. See the parent module's doc comment for the four checks and why each
//! is a warning, never a silent correction.

/// One manufacturability problem found in an (already-solved, for the two
/// mesh-based variants) [`crate::design::Design`].
///
/// See the module docs for what each check looks for. Always a warning to
/// surface to the user, never something this crate acts on itself: `History`
/// stays the sole mutator of `Design`, and nothing in this module touches a
/// `Design` at all.
#[derive(Debug, Clone, PartialEq)]
pub enum ManufacturabilityWarning {
    /// A facet plane this tier describes never reaches the solid's surface --
    /// some later tier's cut removed it entirely. `vanished` counts how many
    /// of the tier's own facet-plane copies (its index-wheel orbit) are
    /// affected; `total` is how many that tier contributes in total (`1` for
    /// a tier with no listed indices).
    VanishingFacet {
        tier_index: usize,
        tier_name: String,
        vanished: usize,
        total: usize,
    },
    /// A facet survives (it has a real polygon on the solid's surface) but
    /// that polygon's area falls under the configured threshold -- see
    /// [`super::DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2`]. `facet_plane_index` is the
    /// index into [`crate::design::Design::planes`]'s combined list, for a caller that
    /// wants to highlight the exact facet (e.g. via
    /// [`indicatrix::geometry::stone_metrics::SolidMesh::rings`]).
    UndersizedFacet {
        tier_index: usize,
        tier_name: String,
        facet_plane_index: usize,
        area: f64,
        threshold: f64,
    },
    /// One of this tier's authored index-wheel positions does not land on a
    /// real gear tooth. `achievable` is the nearest real tooth position;
    /// `azimuth_error_deg` is the resulting azimuthal error if the lapidary
    /// cuts at `achievable` instead of the requested (unreachable) position.
    /// Never silently rounded -- this is reported, not corrected.
    FractionalIndex {
        tier_index: usize,
        tier_name: String,
        requested: f64,
        achievable: f64,
        azimuth_error_deg: f64,
    },
    /// This tier's [`indicatrix::geometry::meet_solver::MeetConstraint::MeetNamed`] resolves
    /// (see [`indicatrix::geometry::meet_solver::MeetNameResolver`]) to a target tier that
    /// comes at or after this
    /// tier's own position in the schedule -- a geometrically valid solve
    /// (the solver has no notion of file order) that is physically
    /// uncuttable in that order, since the target facet doesn't exist yet
    /// when this tier would be cut.
    OutOfOrderMeet {
        tier_index: usize,
        tier_name: String,
        target_tier_index: usize,
        target_tier_name: String,
    },
}

impl std::fmt::Display for ManufacturabilityWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VanishingFacet {
                tier_index,
                tier_name,
                vanished,
                total,
            } => write!(
                f,
                "tier {tier_index} ({tier_name}): {vanished}/{total} facet(s) cut away entirely \
                 by a later tier"
            ),
            Self::UndersizedFacet {
                tier_index,
                tier_name,
                area,
                threshold,
                ..
            } => write!(
                f,
                "tier {tier_index} ({tier_name}): facet area {area:.6} is below the minimum \
                 {threshold:.6}"
            ),
            Self::FractionalIndex {
                tier_index,
                tier_name,
                requested,
                achievable,
                azimuth_error_deg,
            } => write!(
                f,
                "tier {tier_index} ({tier_name}): index {requested} does not land on a gear \
                 tooth; nearest achievable is {achievable} ({azimuth_error_deg:+.4} deg azimuth \
                 error)"
            ),
            Self::OutOfOrderMeet {
                tier_index,
                tier_name,
                target_tier_index,
                target_tier_name,
            } => write!(
                f,
                "tier {tier_index} ({tier_name}): meets tier {target_tier_index} \
                 ({target_tier_name}), which is not cut until later in the schedule"
            ),
        }
    }
}
