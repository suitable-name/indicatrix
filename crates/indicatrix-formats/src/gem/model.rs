//! [`GemDesign`], [`GemFacet`] and the derived [`GemNote`] view.

/// Below this fraction of `|p|`, a facet plane's horizontal component counts as
/// zero: the facet is flat (table or culet) and has no azimuth.
const FLAT_EPS: f64 = 1e-9;

/// Below this fraction of `|p|`, a facet plane's vertical component counts as
/// zero: the facet is exactly vertical (a girdle facet, angle ±90°).
const GIRDLE_EPS: f64 = 1e-12;

/// A tooth position within this distance of 0 or of the tooth count reads as the
/// tooth count itself, as `.asc` files write tooth 0.
const TOOTH_EPS: f64 = 1e-9;

/// One facet record of a `.gem` file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GemFacet {
    /// The plane vector `p`: points `x` on the facet satisfy `p·x = 1`. The unit
    /// outward normal is `p/|p|` and the distance from the centre is `1/|p|`.
    pub plane: [f64; 3],
    /// Tier number. Groups the facets of one tier; 0-based, 1-based or gappy in
    /// real files, so it is a key, not a position.
    pub tier: i32,
    /// The label text before the tab, or `None` when it is empty. `GemCAD` stores
    /// a tier's name on one facet only, the facet its diagram labels.
    pub name: Option<String>,
    /// The label text after the tab: the tier's cutting instructions, usually
    /// repeated on every facet of the tier.
    pub instructions: String,
    /// The facet's polygon as stored. See [`GemDesign::vertex_scale`] before
    /// trusting these: 30 corpus files store them scaled by 0.81.
    pub vertices: Vec<[f64; 3]>,
}

impl GemFacet {
    /// `|p|`, the plane vector's length.
    fn plane_norm(&self) -> f64 {
        let [x, y, z] = self.plane;
        z.mul_add(z, x.mul_add(x, y * y)).sqrt()
    }

    /// The unit outward normal `p/|p|`.
    #[must_use]
    pub fn normal(&self) -> [f64; 3] {
        let norm = self.plane_norm();
        self.plane.map(|c| c / norm)
    }

    /// The plane's distance from the centre, `1/|p|`: the `.asc` mast.
    #[must_use]
    pub fn distance(&self) -> f64 {
        self.plane_norm().recip()
    }

    /// `true` for a facet parallel to the girdle plane (table or culet), which has
    /// no azimuth and hence no meaningful index of its own.
    #[must_use]
    pub fn is_flat(&self) -> bool {
        self.plane[0].hypot(self.plane[1]) <= FLAT_EPS * self.plane_norm()
    }

    /// The facet angle in `.asc`'s signed convention: `+acos(|nz|)` for a crown
    /// facet (`pz > 0`), negative for a pavilion facet (`pz < 0`). A vertical facet
    /// reads exactly `±90`, its side taken from the sign of the tiny stored `pz`
    /// (`.asc` `+90`/`-90`). A culet reads as a sign-negative zero, the table as
    /// `+0.0`, matching [`crate::asc::AscTier::angle_deg`].
    #[must_use]
    pub fn angle_deg(&self) -> f64 {
        let [px, py, pz] = self.plane;
        let norm = self.plane_norm();
        let theta = if pz.abs() <= GIRDLE_EPS * norm {
            90.0
        } else if self.is_flat() {
            0.0
        } else {
            px.hypot(py).atan2(pz.abs()).to_degrees()
        };
        if pz.is_sign_negative() { -theta } else { theta }
    }

    /// The facet's index-wheel tooth for a `gear` and `offset`, from
    /// `phi = atan2(py, px)` (degrees) and `phi = 90° - 360°·(i - offset)/gear`
    /// with the SIGNED gear, solved for `i` and normalised into `(0, |gear|]` so
    /// tooth 0 reads as `|gear|`, as `.asc` files write it. A flat facet has no
    /// azimuth and reads `|gear|`. Returns `0.0` for a zero gear.
    #[must_use]
    pub fn index(&self, gear: i32, offset: f64) -> f64 {
        if gear == 0 {
            return 0.0;
        }
        let teeth = f64::from(gear.unsigned_abs());
        if self.is_flat() {
            return teeth;
        }
        let phi = self.plane[1].atan2(self.plane[0]).to_degrees();
        normalise_tooth(offset + (90.0 - phi) * f64::from(gear) / 360.0, teeth)
    }

    /// The stored vertices divided by `scale`, so that they lie on the plane
    /// `p·x = 1` when `scale` is the file's [`GemDesign::vertex_scale`].
    #[must_use]
    pub fn vertices_on_plane(&self, scale: f64) -> Vec<[f64; 3]> {
        self.vertices.iter().map(|v| v.map(|c| c / scale)).collect()
    }

    /// `p·v` for one vertex.
    pub(super) fn plane_dot(&self, v: [f64; 3]) -> f64 {
        self.plane[2].mul_add(v[2], self.plane[0].mul_add(v[0], self.plane[1] * v[1]))
    }
}

/// Normalises a raw tooth position into `(0, teeth]`, reading a position within
/// [`TOOTH_EPS`] of 0 or `teeth` as `teeth`.
fn normalise_tooth(raw: f64, teeth: f64) -> f64 {
    let r = raw.rem_euclid(teeth);
    if r < TOOTH_EPS || teeth - r < TOOTH_EPS {
        teeth
    } else {
        r
    }
}

/// A derived view of one labelled facet (one with a name or instructions), in the
/// shape the old heuristic text scanner produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GemNote {
    /// Position of the facet in [`GemDesign::facets`].
    pub facet_index: usize,
    /// The facet's name, when it carries one.
    pub facet_name: Option<String>,
    /// The facet's instructions (possibly empty when only a name is present).
    pub text: String,
}

/// A decoded `.gem` design.
#[derive(Debug, Clone, PartialEq)]
pub struct GemDesign {
    /// Every facet record, in file order.
    pub facets: Vec<GemFacet>,
    /// Save-time symmetry order (UI state, like `.asc`'s `y` line).
    pub symmetry: i32,
    /// Mirror flag (`.asc`'s `y`/`n`).
    pub mirror: bool,
    /// Index gear, signed: a negative gear reverses the indexing direction, as
    /// `.asc`'s `g` line does.
    pub gear: i32,
    /// Refractive index (`.asc`'s `I` line).
    pub refractive_index: f64,
    /// Gear offset in teeth (`.asc`'s second `g` field).
    pub gear_offset: f64,
    /// The trailer's unexplained `u32`, `0x7FFF` in every corpus file. Kept raw.
    pub unknown_7fff: u32,
    /// Heading lines H1..H4 (H1 is the title). Empty strings are unused slots.
    pub headings: [String; 4],
    /// Footnote lines F1..F4. Empty strings are unused slots.
    pub footnotes: [String; 4],
    /// The embedded CAM preform design, when the file has a `preform` section.
    pub preform: Option<Box<Self>>,
    /// The median of `p·v` over every stored vertex of this design (not its
    /// preform): `1.0` in 224 corpus files, `0.81` in 30. `1.0` when there are no
    /// vertices. See the module docs for the 0.81 hypothesis.
    pub vertex_scale: f64,
}

impl Default for GemDesign {
    fn default() -> Self {
        Self {
            facets: Vec::new(),
            symmetry: 0,
            mirror: false,
            gear: 0,
            refractive_index: 0.0,
            gear_offset: 0.0,
            unknown_7fff: 0,
            headings: Default::default(),
            footnotes: Default::default(),
            preform: None,
            vertex_scale: 1.0,
        }
    }
}

impl GemDesign {
    /// Every facet that carries a name or instructions, as a [`GemNote`].
    #[must_use]
    pub fn notes(&self) -> Vec<GemNote> {
        self.facets
            .iter()
            .enumerate()
            .filter(|(_, f)| f.name.is_some() || !f.instructions.is_empty())
            .map(|(facet_index, f)| GemNote {
                facet_index,
                facet_name: f.name.clone(),
                text: f.instructions.clone(),
            })
            .collect()
    }

    /// The title (heading H1), when it is not empty.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        Some(self.headings[0].as_str()).filter(|t| !t.is_empty())
    }

    /// `facet`'s tooth under this design's gear and offset; see
    /// [`GemFacet::index`].
    #[must_use]
    pub fn facet_index(&self, facet: &GemFacet) -> f64 {
        facet.index(self.gear, self.gear_offset)
    }

    /// The median of `p·v` over every stored vertex, or `1.0` without vertices.
    pub(super) fn median_vertex_scale(facets: &[GemFacet]) -> f64 {
        let mut dots: Vec<f64> = facets
            .iter()
            .flat_map(|f| f.vertices.iter().map(|&v| f.plane_dot(v)))
            .collect();
        if dots.is_empty() {
            return 1.0;
        }
        dots.sort_by(f64::total_cmp);
        let mid = dots.len() / 2;
        if dots.len().is_multiple_of(2) {
            f64::midpoint(dots[mid - 1], dots[mid])
        } else {
            dots[mid]
        }
    }
}
