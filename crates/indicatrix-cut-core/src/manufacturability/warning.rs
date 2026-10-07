//! [`ManufacturabilityWarning`] itself -- the one type every check reports
//! through. See the parent module's doc comment for the four checks and why each
//! is a warning, never a silent correction.

use crate::design::TierId;

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
        /// The stable id of the tier at `tier_index` when this warning was built
        /// (the tier table badges rows directly instead of parsing 'tier N' out
        /// of the message). See [`Self::tier_id`].
        tier_id: TierId,
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
        /// See [`Self::VanishingFacet`]'s own `tier_id` doc comment.
        tier_id: TierId,
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
        /// See [`Self::VanishingFacet`]'s own `tier_id` doc comment.
        tier_id: TierId,
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
        /// See [`Self::VanishingFacet`]'s own `tier_id` doc comment.
        tier_id: TierId,
        tier_name: String,
        target_tier_index: usize,
        target_tier_name: String,
    },
    /// This tier's [`indicatrix::geometry::meet_solver::MeetConstraint::MeetNamed`]
    /// names at least one target that
    /// [`crate::design::meet_name_is_asc_safe`] rejects -- a name that would not
    /// re-parse back to itself from a plain `.asc` `"Meet <names>"` export (see that
    /// function's own doc comment for exactly which names fail and why). A pure
    /// `.asc` re-export/re-import, or a native load that fell back to
    /// `TierOverlay::SkippedFingerprintMismatch`, loses or mis-targets this meet
    /// reference even though the design solves and exports without any other
    /// complaint today. `unsafe_names` is the exact subset of this tier's authored
    /// names that fail the check, in authored order.
    MeetNameNotAscSafe {
        tier_index: usize,
        /// See [`Self::VanishingFacet`]'s own `tier_id` doc comment.
        tier_id: TierId,
        tier_name: String,
        unsafe_names: Vec<String>,
    },
    /// A concave tool removes nothing: its volume lies outside the stone (or only
    /// grazes it), so the tier cuts no facet there. `tier` indexes
    /// [`crate::design::Design::concave_tiers`] and `placement` that tier's
    /// `indices`; both concave variants below use the same pair.
    ToolMissesStone { tier: usize, placement: usize },
    /// A concave tool cuts clean through the stone: it reaches two flat facets
    /// facing opposite ways, leaving a hole rather than a facet. `facet` is the
    /// lowest-numbered of the touching facets that has such an opposite, as an
    /// index into [`crate::design::Design::planes`]'s combined list. A
    /// reciprocating tool's stroke ends do not count (they leave through the
    /// facets at either end of its axis by design).
    ToolBreaksThrough {
        tier: usize,
        placement: usize,
        facet: usize,
    },
    /// A concave tool cuts away a vertex where three or more of the schedule's flat
    /// facets meet (`vertex` indexes the flat stone's deduplicated vertex list), so
    /// the meet point those facets were cut to no longer exists.
    ToolRemovesMeet {
        tier: usize,
        placement: usize,
        vertex: usize,
    },
    /// Two concave tools remove overlapping volume from the stone. `a` and `b` are
    /// positions in [`crate::design::Design::concave_tools_from_solved`]'s output
    /// (`a < b`), not tier indices; the placement list maps them back.
    ToolsOverlap { a: usize, b: usize },
    /// A facet survives the concave cuts but is a sliver: a flat facet whose area
    /// the tools reduce below the minimum, or a tool surface that small. `facet` is
    /// an index into the flat planes followed by the tools (a tool's facet id is
    /// `planes.len() + k`), the numbering `build_solid_mesh_geom` uses.
    ConcaveSliver { facet: usize },
    /// A concave tool cuts away an extreme vertex of the stone (one that sets its
    /// width, length or height), so the finished stone is smaller than the flat
    /// schedule implies.
    ToolRemovesHullVertex { tier: usize, placement: usize },
    /// A concave tool sits wholly inside the stone: it removes volume but never reaches
    /// a flat facet, so it would be an internal void, which no cutter can reach. The
    /// tool's position or size is wrong (a displacement or depth beyond the stone, or a
    /// tool too small for the depth asked of it).
    ToolEnclosed { tier: usize, placement: usize },
    /// The design has concave tiers and the file format being written (`.asc`,
    /// `.gem`, `.gcs`) cannot represent them, so they are left out of the tier
    /// records (`.asc` keeps two footnotes per tier; the native `.indicatrix` file
    /// stays the lossless carrier). About the design as a whole, so it names no
    /// tier: [`Self::tier_index`] reports `0` and [`Self::tier_id`] `TierId(0)` for
    /// it, and it belongs to the export notice, never to a tier-table row.
    ConcaveTiersOmittedFromExport {
        /// How many concave tiers are left out.
        count: usize,
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
            | Self::OutOfOrderMeet { tier_index, .. }
            | Self::MeetNameNotAscSafe { tier_index, .. } => *tier_index,
            Self::ConcaveTiersOmittedFromExport { .. }
            | Self::ToolMissesStone { .. }
            | Self::ToolBreaksThrough { .. }
            | Self::ToolRemovesMeet { .. }
            | Self::ToolsOverlap { .. }
            | Self::ConcaveSliver { .. }
            | Self::ToolRemovesHullVertex { .. }
            | Self::ToolEnclosed { .. } => 0,
        }
    }

    /// The stable [`TierId`] of the tier this warning is about, as of the solve
    /// this warning was built from -- lets the tier table badge a row directly
    /// by id (stable across add/remove/move/undo) instead of matching
    /// [`Self::tier_index`] against a position that may have shifted since, or
    /// parsing "tier N" back out of [`Self`]'s own `Display` text.
    #[must_use]
    pub const fn tier_id(&self) -> TierId {
        match self {
            Self::VanishingFacet { tier_id, .. }
            | Self::UndersizedFacet { tier_id, .. }
            | Self::FractionalIndex { tier_id, .. }
            | Self::OutOfOrderMeet { tier_id, .. }
            | Self::MeetNameNotAscSafe { tier_id, .. } => *tier_id,
            Self::ConcaveTiersOmittedFromExport { .. }
            | Self::ToolMissesStone { .. }
            | Self::ToolBreaksThrough { .. }
            | Self::ToolRemovesMeet { .. }
            | Self::ToolsOverlap { .. }
            | Self::ConcaveSliver { .. }
            | Self::ToolRemovesHullVertex { .. }
            | Self::ToolEnclosed { .. } => TierId(0),
        }
    }

    /// The concave tier (index into [`crate::design::Design::concave_tiers`]) this
    /// warning is about, for the variants that name one. [`Self::tier_index`] and
    /// [`Self::tier_id`] are about flat tiers and report `0` for these, so a
    /// tier-table row never inherits a concave tool's warning by accident.
    #[must_use]
    pub const fn concave_tier(&self) -> Option<usize> {
        match self {
            Self::ToolMissesStone { tier, .. }
            | Self::ToolBreaksThrough { tier, .. }
            | Self::ToolRemovesMeet { tier, .. }
            | Self::ToolRemovesHullVertex { tier, .. }
            | Self::ToolEnclosed { tier, .. } => Some(*tier),
            Self::VanishingFacet { .. }
            | Self::UndersizedFacet { .. }
            | Self::FractionalIndex { .. }
            | Self::OutOfOrderMeet { .. }
            | Self::MeetNameNotAscSafe { .. }
            | Self::ConcaveTiersOmittedFromExport { .. }
            | Self::ToolsOverlap { .. }
            | Self::ConcaveSliver { .. } => None,
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
            }
            | Self::ToolBreaksThrough {
                facet: facet_plane_index,
                ..
            } => Some(*facet_plane_index),
            Self::VanishingFacet { .. }
            | Self::FractionalIndex { .. }
            | Self::OutOfOrderMeet { .. }
            | Self::MeetNameNotAscSafe { .. }
            | Self::ConcaveTiersOmittedFromExport { .. }
            | Self::ToolMissesStone { .. }
            | Self::ToolRemovesMeet { .. }
            | Self::ToolsOverlap { .. }
            | Self::ConcaveSliver { .. }
            | Self::ToolRemovesHullVertex { .. }
            | Self::ToolEnclosed { .. } => None,
        }
    }
}

impl std::fmt::Display for ManufacturabilityWarning {
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per warning variant; splitting would only scatter the user-facing texts"
    )]
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
                ..
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
                ..
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
                ..
            } => write!(
                f,
                "tier {} ({tier_name}): meets tier {} \
                 ({target_tier_name}), which is not cut until later in the schedule",
                tier_index + 1,
                target_tier_index + 1
            ),
            Self::MeetNameNotAscSafe {
                tier_index,
                tier_name,
                unsafe_names,
                ..
            } => write!(
                f,
                "tier {} ({tier_name}): meet target name(s) {} would not survive a plain \
                 .asc export/re-import -- rename without spaces, commas or semicolons, and \
                 without leading/trailing punctuation",
                tier_index + 1,
                unsafe_names.join(", ")
            ),
            Self::ToolMissesStone { tier, placement } => write!(
                f,
                "concave tier {} placement {}: the tool lies outside the stone and cuts nothing",
                tier + 1,
                placement + 1
            ),
            Self::ToolBreaksThrough {
                tier,
                placement,
                facet,
            } => write!(
                f,
                "concave tier {} placement {}: the tool cuts clean through the stone \
                 (it reaches facet {} and the facet opposite it)",
                tier + 1,
                placement + 1,
                facet + 1
            ),
            Self::ToolRemovesMeet {
                tier,
                placement,
                vertex,
            } => write!(
                f,
                "concave tier {} placement {}: the tool cuts away the meet point of vertex {}",
                tier + 1,
                placement + 1,
                vertex + 1
            ),
            Self::ToolsOverlap { a, b } => write!(
                f,
                "concave tools {} and {} remove overlapping volume",
                a + 1,
                b + 1
            ),
            Self::ConcaveSliver { facet } => write!(
                f,
                "facet {} is left a sliver by the concave cuts",
                facet + 1
            ),
            Self::ToolRemovesHullVertex { tier, placement } => write!(
                f,
                "concave tier {} placement {}: the tool cuts away a vertex that sets the \
                 stone's width, length or height",
                tier + 1,
                placement + 1
            ),
            Self::ToolEnclosed { tier, placement } => write!(
                f,
                "concave tier {} placement {}: the tool sits entirely inside the stone and never \
                 reaches its surface, so it cannot be cut",
                tier + 1,
                placement + 1
            ),
            Self::ConcaveTiersOmittedFromExport { count } => write!(
                f,
                "{count} concave tier(s) cannot be written to this format and are left out of \
                 the tier list; the .asc keeps them as footnotes, the .indicatrix file keeps \
                 them in full"
            ),
        }
    }
}
