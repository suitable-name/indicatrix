//! [`GcsTier`] and its solved-geometry children [`GcsFacet`]/[`GcsVertex`].

/// Below this, a normal's horizontal component counts as zero: the facet is flat
/// (table or culet) and has no azimuth.
const FLAT_EPS: f64 = 1e-9;

/// Above this, a normal's `nz` makes a facet a crown facet; anything else (the
/// pavilion and the vertical girdle) uses the pavilion index winding.
const CROWN_EPS: f64 = 1e-9;

/// A tooth position within this distance of 0 or of the tooth count reads as the
/// tooth count itself.
const TOOTH_EPS: f64 = 1e-9;

/// One `<vertex .../>` point of a solved facet's polygon outline.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GcsVertex {
    /// X coordinate, in the design's normalised frame (`max(|x|,|y|) = 1`).
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
    /// Z coordinate (the stone's optical axis, positive up, z-range centred on 0).
    pub z: f64,
}

/// The `index_angle` Gem Cut Studio writes for a facet with outward `normal`.
///
/// `90° + phi` on the crown, `270° - phi` on the pavilion and girdle, with
/// `phi = atan2(ny, nx)`, reduced into `[0, 360)`. `None` for a flat facet, whose
/// `index_angle` is arbitrary.
///
/// Measured on 4,836 of 4,836 non-flat corpus facets (2,159 crown, 2,677
/// pavilion/girdle); the manual states only that "pav vs crown has opposite index
/// ordering" (Gem Cut Studio User's Manual v1.1.0, p.59). The corpus has no chiral
/// design, so the rule is verified on labels and normals, not on a design where
/// crown and pavilion winding interact visibly.
#[must_use]
pub fn side_rule_index_angle(normal: [f64; 3]) -> Option<f64> {
    let [nx, ny, nz] = normal;
    if nx.hypot(ny) <= FLAT_EPS {
        return None;
    }
    let phi = ny.atan2(nx).to_degrees();
    let raw = if nz > CROWN_EPS {
        90.0 + phi
    } else {
        270.0 - phi
    };
    Some(raw.rem_euclid(360.0))
}

/// The unit outward normal of a facet at polar `angle_deg` and `index_angle_deg`.
///
/// The inverse of [`side_rule_index_angle`] (polar angle 0 table, 90 girdle, 180
/// culet). The azimuth is `index_angle - 90°` on the crown
/// (`angle_deg < 90`) and `270° - index_angle` otherwise.
#[must_use]
pub fn normal_from_index_angle(angle_deg: f64, index_angle_deg: f64) -> [f64; 3] {
    let phi = if angle_deg < 90.0 {
        index_angle_deg - 90.0
    } else {
        270.0 - index_angle_deg
    }
    .to_radians();
    let theta = angle_deg.to_radians();
    [
        theta.sin() * phi.cos(),
        theta.sin() * phi.sin(),
        theta.cos(),
    ]
}

/// One solved facet plane: a polygon at one azimuthal ("index") position within a
/// [`GcsTier`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GcsFacet {
    /// The facet plane's outward unit normal `(nx, ny, nz)`. Optional in the file
    /// (derivable "from the tier angle and `index_angle`", manual p.59); when
    /// absent, the parser derives it with [`normal_from_index_angle`].
    pub normal: [f64; 3],
    /// The index position in absolute degrees: `tooth / gear · 360`. Crown and
    /// pavilion wind in opposite directions relative to the normal's azimuth (see
    /// [`side_rule_index_angle`]); a flat facet (table, culet) carries an arbitrary
    /// value (`0`, `45`, ...). When absent, the parser derives it from the normal.
    pub index_angle_deg: f64,
    /// Per-facet frosting, when the file states it (`frosting="0.5"` = frosted).
    /// The attribute name is taken from third-party writers (`GCSViewer`); the
    /// manual only names the tier-level attribute.
    pub frosting: Option<f64>,
    /// The facet's polygon outline, in file order. Optional in the file.
    pub vertices: Vec<GcsVertex>,
}

impl GcsFacet {
    /// This facet's index-wheel tooth for a `gear`-tooth wheel, normalised into
    /// `(0, gear]` so tooth 0 reads as `gear` (as `.asc` files write it).
    ///
    /// For a non-flat facet the tooth comes from the normal through
    /// [`side_rule_index_angle`] (it equals the stored `index_angle` on every
    /// corpus facet); a flat facet has no azimuth and uses the stored
    /// [`Self::index_angle_deg`]. Returns `0.0` for a zero gear.
    #[must_use]
    pub fn index(&self, gear: u32) -> f64 {
        if gear == 0 {
            return 0.0;
        }
        let teeth = f64::from(gear);
        let angle = side_rule_index_angle(self.normal).unwrap_or(self.index_angle_deg);
        let r = (angle * teeth / 360.0).rem_euclid(teeth);
        if r < TOOTH_EPS || teeth - r < TOOTH_EPS {
            teeth
        } else {
            r
        }
    }

    /// `true` when [`Self::frosting`] is present and positive.
    #[must_use]
    pub fn is_frosted(&self) -> bool {
        self.frosting.is_some_and(|f| f > 0.0)
    }
}

/// One `<tier>...</tier>` block: a single angle/depth setting, realised as one
/// solved facet per index position.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GcsTier {
    /// Polar angle in degrees: `0` table, `90` girdle ("always included in pav"),
    /// above 90 pavilion, `180` a flat culet (manual p.58). See
    /// [`Self::to_signed_asc_angle`].
    pub angle_deg: f64,
    /// Plane distance from the centre in Gem Cut Studio's normalised frame (scaled
    /// so `max(|x|,|y|) = 1`, z-range centred on 0). This is the `.asc` mast after
    /// that normalisation: `depth = s·|mast| - s·z0·nz`. Optional in the file when
    /// vertices are present (manual p.58); the parser then takes the mean `n·v` over
    /// the tier's vertices.
    pub depth: f64,
    /// This tier's facet name, e.g. `"P1"`, `"C7"`, `"G1"`, `"T"`. Stored exactly as
    /// written.
    pub name: String,
    /// Free-text cutting instructions, in the same domain as
    /// [`crate::asc::AscTier::notes`]. Raw CR/LF inside the value are kept.
    pub instructions: String,
    /// Show/Hide: a hidden tier is excluded from Gem Cut Studio's raytrace (manual
    /// p.9). Always `true` in the corpus.
    pub visible: bool,
    /// Undocumented; probably the "Preform" tier flag (manual p.9). Always `false`
    /// in the corpus.
    pub guide: bool,
    /// The "Frosted" tier toggle, when stated: `0` clear, `0.5` frosted as Gem Cut
    /// Studio 1.1 writes it (manual p.9 and its p.58 example `frosting="0"`).
    pub frosting: Option<f64>,
    /// Every solved facet at this tier's angle/depth, one per index position.
    pub facets: Vec<GcsFacet>,
}

impl GcsTier {
    /// Converts [`Self::angle_deg`] to `.asc`'s signed convention (negative =
    /// pavilion, non-negative = crown): identity below 90, `-(180 - angle_deg)`
    /// from 90 up. The girdle (`90`) therefore reads `-90`: the manual puts it on the
    /// pavilion side ("always included in pav", p.58), and its facets use the
    /// pavilion index winding.
    ///
    /// Written as `-(180.0 - angle_deg)` so that a 180-degree culet signs its zero
    /// NEGATIVE (`-0.0`): `.asc` tells a crown table (`0.0`) from a pavilion culet
    /// (`-0.0`) by the sign bit alone (see [`crate::asc::AscTier::angle_deg`]).
    #[must_use]
    pub fn to_signed_asc_angle(&self) -> f64 {
        if self.angle_deg < 90.0 {
            self.angle_deg
        } else {
            -(180.0 - self.angle_deg)
        }
    }

    /// Total number of solved facet planes in this tier.
    #[must_use]
    pub const fn facet_plane_count(&self) -> usize {
        self.facets.len()
    }

    /// `true` when [`Self::frosting`] is present and positive.
    #[must_use]
    pub fn is_frosted(&self) -> bool {
        self.frosting.is_some_and(|f| f > 0.0)
    }
}
