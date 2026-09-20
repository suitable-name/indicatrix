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
    /// [`indicatrix::geometry::stone_metrics::SolidMesh::rings`]). `area` and
    /// `threshold` stay in the design's own arbitrary "mast unit" scale (never
    /// meaningful to a cutter on their own -- see [`Self`]'s `Display` impl,
    /// which reports both as a fraction of `width_axis` instead).
    UndersizedFacet {
        tier_index: usize,
        tier_name: String,
        facet_plane_index: usize,
        area: f64,
        threshold: f64,
        /// The solid's own measured width along its widest axis, in the same
        /// "mast unit" scale as `area`/`threshold` -- lets [`Self`]'s `Display`
        /// impl report the facet's size as a cutter-meaningful percentage of
        /// stone width (`sqrt(area) / width_axis`) instead of a raw area no
        /// design-independent number could ever be compared against.
        width_axis: f64,
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

impl ManufacturabilityWarning {
    /// The tier this warning is about -- every variant carries one. Lets a caller
    /// (e.g. the editor's tier table) attribute a warning to a row without
    /// matching on the specific variant itself -- see "warnings are not
    /// attributed to rows or facets": today a warning only ever reaches the UI
    /// flattened to a `String` (via `Display`, below), with nothing keeping this
    /// index alongside the text for a caller that wants to badge a row.
    #[must_use]
    pub const fn tier_index(&self) -> usize {
        match self {
            Self::VanishingFacet { tier_index, .. }
            | Self::UndersizedFacet { tier_index, .. }
            | Self::FractionalIndex { tier_index, .. }
            | Self::OutOfOrderMeet { tier_index, .. } => *tier_index,
        }
    }

    /// The facet-plane index a caller should highlight in the 3D view, when this
    /// warning names one -- only [`Self::UndersizedFacet`] does; every other
    /// variant is about a tier as a whole (a vanished facet has no surviving
    /// polygon left to highlight, and the index/cut-order checks are about
    /// authored state, not a specific facet).
    #[must_use]
    pub const fn facet_plane_index(&self) -> Option<usize> {
        match self {
            Self::UndersizedFacet {
                facet_plane_index, ..
            } => Some(*facet_plane_index),
            Self::VanishingFacet { .. }
            | Self::FractionalIndex { .. }
            | Self::OutOfOrderMeet { .. } => None,
        }
    }
}

impl std::fmt::Display for ManufacturabilityWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Every tier number below is `+ 1`: `tier_index`/`target_tier_index` stay
        // 0-based (this crate's tier list is always indexed that way, and
        // `Self::tier_index` must keep returning the raw index so a caller can use
        // it to index `Design::tiers`), but the tier TABLE the cutter actually
        // reads numbers its rows from 1 -- these strings are user-facing text, not
        // an index, so they follow the table rather than the storage convention.
        match self {
            Self::VanishingFacet {
                tier_index,
                tier_name,
                vanished,
                total,
            } => write!(
                f,
                "tier {} ({tier_name}): {vanished}/{total} facet(s) cut away entirely \
                 by a later tier",
                tier_index + 1
            ),
            Self::UndersizedFacet {
                tier_index,
                tier_name,
                area,
                threshold,
                width_axis,
                ..
            } => {
                // Both expressed as a percentage of the stone's own measured width
                // (`sqrt(area) / width_axis`) rather than the raw, design-scale
                // area/threshold: a cutter has no way to judge whether `0.000241`
                // "mast units squared" is a sliver or a real facet, but "0.4% of
                // stone width" (below a 1% minimum) means something on any design.
                let facet_pct = if *width_axis > 0.0 {
                    100.0 * area.sqrt() / width_axis
                } else {
                    f64::NAN
                };
                let threshold_pct = if *width_axis > 0.0 {
                    100.0 * threshold.sqrt() / width_axis
                } else {
                    f64::NAN
                };
                write!(
                    f,
                    "tier {} ({tier_name}): facet spans only {facet_pct:.2}% of the stone's \
                     width, below the {threshold_pct:.2}% minimum",
                    tier_index + 1
                )
            }
            Self::FractionalIndex {
                tier_index,
                tier_name,
                requested,
                achievable,
                azimuth_error_deg,
            } => write!(
                f,
                "tier {} ({tier_name}): index {requested} does not land on a gear \
                 tooth; nearest achievable is {achievable} ({azimuth_error_deg:+.4} deg azimuth \
                 error)",
                tier_index + 1
            ),
            Self::OutOfOrderMeet {
                tier_index,
                tier_name,
                target_tier_index,
                target_tier_name,
            } => write!(
                f,
                "tier {} ({tier_name}): meets tier {} \
                 ({target_tier_name}), which is not cut until later in the schedule",
                tier_index + 1,
                target_tier_index + 1
            ),
        }
    }
}
