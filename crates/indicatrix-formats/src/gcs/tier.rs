//! [`GcsTier`] and its solved-geometry children [`GcsFacet`]/[`GcsVertex`].

/// One `<vertex .../>` point of a solved facet's polygon outline.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GcsVertex {
    /// X coordinate, in the design's normalized (reference-radius-1) frame.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
    /// Z coordinate (the stone's optical axis).
    pub z: f64,
}

/// One solved facet plane: a real polygon at one azimuthal ("index") position
/// within a [`GcsTier`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GcsFacet {
    /// The facet plane's outward unit normal `(nx, ny, nz)`.
    pub normal: [f64; 3],
    /// This facet's azimuthal position, in degrees. Confirmed (see module docs)
    /// to equal `(tooth % gear) / gear * 360.0` for `.asc`'s corresponding tooth
    /// number on every non-flat tier checked -- but see [`GcsTier`]'s docs for the
    /// one confirmed exception (a tier with a single, full-width facet, e.g. the
    /// table, where this value is not meaningful).
    pub index_angle_deg: f64,
    /// The facet's polygon outline, in file order.
    pub vertices: Vec<GcsVertex>,
}

/// One `<tier>...</tier>` block: a single angle/depth setting, realized as one
/// solved facet per azimuthal ("index") position.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GcsTier {
    /// Polar angle in degrees, `0` at the crown apex (table-normal direction)
    /// through `90` at the girdle plane (vertical) to `180` at the culet
    /// direction. See the module docs for the verified relationship to
    /// [`crate::asc::AscTier::angle_deg`]'s signed convention.
    pub angle_deg: f64,
    /// Distance from the design's center in its own normalized frame. **Not**
    /// directly comparable to `.asc`'s `mast` -- see the module docs.
    pub depth: f64,
    /// This tier's facet name, e.g. `"P1"`, `"C7"`, `"G1"`, `"T"`. Stored exactly
    /// as written; often does not match the name a sibling `.asc` uses for the
    /// same physical facet (the two formats appear to name tiers independently --
    /// see the module docs' angle-based, not name-based, verification method).
    pub name: String,
    /// Free-text cutting/meet instruction, in the same domain as
    /// [`crate::asc::AscTier::notes`] / [`crate::asc::MeetInstruction`] (real
    /// corpus values include `"Meet C1, C2, and C3"`, `"Level girdle"`, `"Cut to
    /// CP"`, and `"CAM PF1  35.760  64-16-32-48"`-style compound-angle-mast
    /// notes). Empty when the file leaves it blank, which is the common case.
    pub instructions: String,
    /// Whether Gem Cut Studio currently displays this tier. Always `true` in the
    /// sampled corpus, but the attribute is real and is not discarded.
    pub visible: bool,
    /// Whether this tier is a non-physical construction/guide line rather than a
    /// real cut facet. Always `false` in the sampled corpus (no real example to
    /// verify the `true` case's other fields against), but the attribute is real
    /// and is not discarded.
    pub guide: bool,
    /// Every solved facet at this tier's angle/depth, one per azimuthal position.
    pub facets: Vec<GcsFacet>,
}

impl GcsTier {
    /// Converts [`Self::angle_deg`] to `.asc`'s signed convention (negative =
    /// pavilion, non-negative = crown): identity for `angle_deg <= 90.0`,
    /// `angle_deg - 180.0` otherwise. Verified against 44 of 56 real `.gcs`/`.asc`
    /// sibling pairs in the corpus -- see the module docs for the other 12.
    #[must_use]
    pub fn to_signed_asc_angle(&self) -> f64 {
        if self.angle_deg <= 90.0 {
            self.angle_deg
        } else {
            self.angle_deg - 180.0
        }
    }

    /// Total number of solved facet planes in this tier (one per azimuthal
    /// position).
    #[must_use]
    pub const fn facet_plane_count(&self) -> usize {
        self.facets.len()
    }
}
