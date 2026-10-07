//! Built-in "New Design" templates.
//!
//! A small, curated gallery of classic faceting designs, each described by a
//! static [`TemplateSpec`] entry in [`TEMPLATES`] instead of hard-coded in
//! `apps/indicatrix-cut/src/gui/editor/callbacks/tier_actions/new_design.rs`'s
//! `do_new_design_create`.
//!
//! Every template here is built from the exact same proven, tested topology
//! [`crate::design::ConstraintTier::standard_round_brilliant`] already uses:
//! a 96-tooth index gear, 8-fold mirrored symmetry, and every tier pinned via
//! [`indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference`] (an
//! authored exact depth), never a [`indicatrix::geometry::meet_solver::MeetConstraint::MeetNamed`]/
//! `MeetExisting` reference to another tier. That restriction is deliberate: this
//! module's own tests are the only verification these designs get before shipping
//! (one batched `cargo test` run, not iterative solver-tuning), and a
//! `ScaleReference`-only design's
//! closure depends only on its own masts, never on another tier's solved
//! position, so it is the one construction this crate's test suite can verify
//! with confidence in a single pass.
//!
//! **Scope note (deliberately not "a dozen classic designs"):** this module ships
//! nine designs. The "top five shapes" (the standard round brilliant, an oval, a
//! cushion, an emerald step cut and a princess) form the featured group; the
//! round-brilliant variants and the two teaching designs form the second group.
//! The four newer shapes are authored as plain tier tables (`Row`) whose masts
//! were fitted so that every facet is alive, no facet is too small to cut, and the
//! outline has the intended length-to-width ratio -- on the template's own default
//! preform AND after a remap to each gear [`TemplateSpec::allowed_gears_among`]
//! offers. A trillion (3-fold layout) is not shipped: it needs an unproven
//! gear/symmetry/index combination. See this crate's own `#[cfg(test)]` module
//! below for the verification every entry gets.
//!
//! # Tier order
//!
//! Every tier list is stored in the order it is cut ([`crate::design::Design::cutting_order`]):
//! the pavilion mains, the girdle, the lower girdle and the culet, then the crown mains, the
//! stars and the upper girdle, and the table last, so the tier table, the Cut slider and the
//! printed cutting sheet all start with the same tier. The emerald step cut keeps its
//! authored order inside each section (its cutting order already puts the pavilion first).
//! Only [`crate::design::ConstraintTier::standard_round_brilliant`], the fixture about 160
//! tests build on, stays top-down.
//!
//! # Metadata
//!
//! Besides the tier table, a [`TemplateSpec`] names its outline family
//! ([`ShapeFamily`]), whether it is one of the featured shapes, the refractive
//! index its angles were designed for ([`TemplateSpec::design_ri`]) together with the
//! built-in material that index belongs to ([`TemplateSpec::default_material`]),
//! its default preform, and -- derived, not listed -- the gears it can be remapped
//! onto ([`TemplateSpec::allows_gear`]).
//!
//! The templates are only ever APPENDED to [`TEMPLATES`]: the position in that table is
//! the template's identity for the web app, the worked-example guide and the saved
//! test fixtures (`EditorSession::from_template`'s index is `position + 1`).

use crate::{
    design::{
        ConcaveTier, ConcaveTool, ConstraintTier, Design, FreshDesignSpec, ScheduleMeta, ToolMotion,
    },
    edit::remap_ratio,
    material::MaterialSelection,
    preform::PreformSpec,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// The outline family a template belongs to -- what the gallery and the shape
/// verdicts group by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShapeFamily {
    /// Round brilliant and its variants and teaching designs.
    Round,
    /// An elongated brilliant.
    Oval,
    /// A rounded-square brilliant.
    Cushion,
    /// A rectangular step cut with cut corners.
    Emerald,
    /// A square modified brilliant.
    Princess,
}

impl ShapeFamily {
    /// The family's display name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Round => "Round",
            Self::Oval => "Oval",
            Self::Cushion => "Cushion",
            Self::Emerald => "Emerald",
            Self::Princess => "Princess",
        }
    }
}

/// One authored tier of a table-driven template: `(name, angle_deg, indices, mast)`.
type Row = (&'static str, f64, &'static [f64], f64);

/// One gallery entry: display metadata plus a pure constructor for its tier table.
///
/// `build` is a plain function pointer (not a closure) so [`TEMPLATES`]
/// can be a `const` table -- the same "static data, not a builder object" shape
/// [`crate::design::ConstraintTier::standard_round_brilliant`] already uses for
/// the one template that predates this module.
#[derive(Debug, Clone, Copy)]
pub struct TemplateSpec {
    /// Shown as the gallery card's title and the "Start From" selection text.
    pub name: &'static str,
    /// A short shape/fold description, e.g. "8-fold round brilliant" -- shown
    /// under the name on the gallery card, distinct from `description` (a
    /// full sentence) so a narrow card can show just this line.
    pub shape: &'static str,
    /// The index gear tooth count this template's `indices` were authored for.
    pub gear_teeth: i32,
    /// The symmetry order (repeat count) this template's `indices` were
    /// authored for.
    pub symmetry_order: u32,
    /// Whether this template's repeats are also mirrored.
    pub mirror: bool,
    /// One sentence describing the design, shown on hover/selection.
    pub description: &'static str,
    /// The outline family -- see [`ShapeFamily`].
    pub family: ShapeFamily,
    /// Whether this is one of the "top five shapes" the New Design dialog lists first;
    /// the others (round-brilliant variants and teaching designs) form its second group.
    pub featured: bool,
    /// The refractive index (`n_D`) the facet angles were designed for. Creating the
    /// design in a material whose index differs adapts the angles
    /// (`indicatrix_editor::templates::create_from_template`).
    pub design_ri: f64,
    /// The built-in material whose `n_D` is [`Self::design_ri`] -- the material a new
    /// design from this template starts in.
    pub default_material: &'static str,
    /// The preform the dialog cuts this template from. Tall enough that the finished
    /// stone is never clipped by the rough's own bottom or walls.
    pub preform: PreformSpec,
    /// Builds this template's tier table. Called once per selection, not
    /// cached -- these are all cheap, small `Vec` literals (the same cost
    /// [`crate::design::ConstraintTier::standard_round_brilliant`] already pays
    /// on every "Create").
    build: fn() -> Vec<ConstraintTier>,
}

/// How close two gear positions must be to a half tooth for the remap to count as a
/// tie (a tie rounds one way for an index and the other for its mirror image).
const TIE_EPSILON: f64 = 1e-9;

impl TemplateSpec {
    /// The fresh-design spec for this template at its own gear, symmetry and mirror, on
    /// its default preform, started in `material`.
    #[must_use]
    pub const fn fresh_spec(&self, material: MaterialSelection) -> FreshDesignSpec {
        FreshDesignSpec {
            gear_teeth: self.gear_teeth,
            symmetry_order: self.symmetry_order,
            mirror: self.mirror,
            material,
            preform: self.preform,
        }
    }

    /// The material selection a new design from this template starts with: the
    /// [`Self::default_material`] by name, nothing overridden.
    #[must_use]
    pub fn default_material_selection(&self) -> MaterialSelection {
        MaterialSelection {
            name: Some(self.default_material.to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        }
    }

    /// Whether this template's index set can be remapped onto `gear` teeth by symmetry:
    ///
    /// - `gear` is the template's own gear (always allowed), or
    /// - `gear` is a multiple of the symmetry order (so the repeats still land on whole
    ///   teeth), no scaled position falls exactly half way between two teeth (such a tie
    ///   rounds an index and its mirror image differently and breaks the mirror), and no
    ///   two positions of one tier land on the same tooth.
    ///
    /// The rule uses the same arithmetic as `Edit::RemapIndices` with nearest rounding.
    /// Whether the remapped stone still has every facet is checked by this module's
    /// tests for each template and each gear the dialog offers, not decided here.
    #[must_use]
    pub fn allows_gear(&self, gear: i32) -> bool {
        if gear == self.gear_teeth {
            return true;
        }
        let Ok(teeth) = u32::try_from(gear) else {
            return false;
        };
        if teeth == 0 || self.symmetry_order == 0 || !teeth.is_multiple_of(self.symmetry_order) {
            return false;
        }
        let ratio = remap_ratio(self.gear_teeth, gear);
        let wheel = f64::from(teeth);
        self.tiers().iter().all(|tier| {
            let mut landed: Vec<u64> = Vec::with_capacity(tier.indices.len());
            for &index in &tier.indices {
                let scaled = index * ratio;
                if (scaled - scaled.floor() - 0.5).abs() < TIE_EPSILON {
                    return false;
                }
                // `+ 0.0` turns a negative zero into a plain one so equal teeth compare equal.
                let tooth = (scaled.round().rem_euclid(wheel) + 0.0).to_bits();
                if landed.contains(&tooth) {
                    return false;
                }
                landed.push(tooth);
            }
            true
        })
    }

    /// The gears from `candidates` this template can be remapped onto
    /// ([`Self::allows_gear`]), in the order given, always starting with the template's
    /// own gear.
    #[must_use]
    pub fn allowed_gears_among(&self, candidates: &[i32]) -> Vec<i32> {
        let mut gears = vec![self.gear_teeth];
        for &gear in candidates {
            if !gears.contains(&gear) && self.allows_gear(gear) {
                gears.push(gear);
            }
        }
        gears
    }

    /// Builds this template's tier table -- see [`Self::build`]'s own doc
    /// comment for why this is a method wrapping a function pointer rather than
    /// the field being called directly (a private field needs an accessor).
    #[must_use]
    pub fn tiers(&self) -> Vec<ConstraintTier> {
        (self.build)()
    }

    /// The schedule metadata (`gear_teeth`/`symmetry_order`/`mirror`) this
    /// template's own `indices` were authored to match, paired with
    /// [`Self::tiers`] the same way [`ScheduleMeta::standard_round_brilliant`]
    /// pairs with [`ConstraintTier::standard_round_brilliant`].
    #[must_use]
    pub fn schedule_meta(&self) -> ScheduleMeta {
        ScheduleMeta {
            // Just the version number -- see `ConstraintTier::standard_round_brilliant`'s
            // matching field for why the `"GemCad "` prefix does not belong here
            // too (`indicatrix_formats::asc::to_asc_string` already writes it).
            gemcad_version: "5.0".to_string(),
            gear_teeth: self.gear_teeth,
            gear_reference_angle: 0.0,
            symmetry_order: self.symmetry_order,
            mirror: self.mirror,
            refractive_index: 1.54,
            headers: Vec::new(),
            footnotes: Vec::new(),
        }
    }
}

/// Index-wheel positions shared by every template in this module -- the same
/// 96-tooth, 8-fold layout [`ConstraintTier::standard_round_brilliant`] uses,
/// reproduced here (rather than exported from that function, which keeps its
/// own copies private) since every template below needs at least one of them.
mod indices {
    /// The 16 girdle-band facet positions on a 96-tooth wheel.
    pub const GIRDLE: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];
    /// The 16 upper-/lower-girdle ("break") facet positions on a 96-tooth wheel.
    pub const BREAK: [f64; 16] = [
        95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0, 83.0,
        85.0,
    ];
    /// The 8 crown-/pavilion-main facet positions on a 96-tooth wheel.
    pub const MAIN: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    /// The 8 star facet positions on a 96-tooth wheel.
    pub const STAR: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];
}

/// Builds one `ScaleReference`-pinned tier -- the same small helper
/// [`ConstraintTier::standard_round_brilliant`] declares privately for itself,
/// reproduced here for the same reason [`indices`] is: every template below
/// needs it and that copy is not exported.
fn scale_tier(name: &str, angle_deg: f64, indices: &[f64], mast: f64) -> ConstraintTier {
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

/// Every template is stored in the order it is cut ([`crate::design::Design::cutting_order`]):
/// the pavilion mains, the girdle, the lower girdle and the culet, then the crown mains, the
/// stars and the upper girdle, and the table last. The tiers, angles, indices and masts are
/// the ones [`ConstraintTier::standard_round_brilliant`] has; only the order differs, so the
/// first step a new design shows is the first step a cutter takes. (The fixture itself stays
/// top-down: about 160 tests build on it.)
fn round_brilliant_tiers() -> Vec<ConstraintTier> {
    vec![
        scale_tier("Pavilion Main", -41.0, &indices::MAIN, 0.67),
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        scale_tier("Lower Girdle", -42.5, &indices::BREAK, 0.68),
        scale_tier("Culet", -0.0, &[], 0.88),
        scale_tier("Crown Main", 34.5, &indices::MAIN, 0.59),
        scale_tier("Star", 15.0, &indices::STAR, 0.45),
        scale_tier("Upper Girdle", 41.0, &indices::BREAK, 0.67),
        scale_tier("Table", 0.0, &[], 0.32),
    ]
}

/// A shallower pavilion (mains at -38 instead of -41, lower girdle following at
/// -40) -- closes the same way the standard template does, but with less
/// pavilion angle margin over a typical critical angle, useful for
/// demonstrating windowing risk in the guide/inspector's margin readout.
fn shallow_pavilion_tiers() -> Vec<ConstraintTier> {
    vec![
        scale_tier("Pavilion Main", -38.0, &indices::MAIN, 0.67),
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        scale_tier("Lower Girdle", -40.0, &indices::BREAK, 0.68),
        scale_tier("Culet", -0.0, &[], 0.88),
        scale_tier("Crown Main", 34.5, &indices::MAIN, 0.59),
        scale_tier("Star", 15.0, &indices::STAR, 0.45),
        scale_tier("Upper Girdle", 41.0, &indices::BREAK, 0.67),
        scale_tier("Table", 0.0, &[], 0.32),
    ]
}

/// A deeper pavilion (mains at -43.5, lower girdle at -44.5) -- closer to a
/// typical critical angle than the standard template, useful for demonstrating
/// a "Marginal"/"Windows" risk band in the guide/inspector.
fn deep_pavilion_tiers() -> Vec<ConstraintTier> {
    vec![
        scale_tier("Pavilion Main", -43.5, &indices::MAIN, 0.67),
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        scale_tier("Lower Girdle", -44.5, &indices::BREAK, 0.68),
        scale_tier("Culet", -0.0, &[], 0.88),
        scale_tier("Crown Main", 34.5, &indices::MAIN, 0.59),
        scale_tier("Star", 15.0, &indices::STAR, 0.45),
        scale_tier("Upper Girdle", 41.0, &indices::BREAK, 0.67),
        scale_tier("Table", 0.0, &[], 0.32),
    ]
}

/// The simplest possible closed design on this layout: a table, a girdle band,
/// and one ring of pavilion mains -- no crown facets, no star/break rings. Three
/// tiers total, meant as a first design smaller than the standard round
/// brilliant's eight, for a cutter learning the tier table. Stored in cutting
/// order: pavilion main, girdle, table.
fn simple_teaching_tiers() -> Vec<ConstraintTier> {
    vec![
        scale_tier("Pavilion Main", -40.0, &indices::MAIN, 0.70),
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        scale_tier("Table", 0.0, &[], 0.30),
    ]
}

/// One step up from [`simple_teaching_tiers`]: adds a single crown-main ring
/// above the girdle, so both blocks have one real facet ring each -- pavilion
/// main, girdle, crown main, table (the order they are cut in). Four tiers, still
/// no star/break rings.
fn rich_teaching_tiers() -> Vec<ConstraintTier> {
    vec![
        scale_tier("Pavilion Main", -40.0, &indices::MAIN, 0.70),
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        scale_tier("Crown Main", 34.5, &indices::MAIN, 0.60),
        scale_tier("Table", 0.0, &[], 0.30),
    ]
}

/// Turns a table of [`Row`]s into pinned tiers.
fn tiers_from_rows(rows: &[Row]) -> Vec<ConstraintTier> {
    rows.iter()
        .map(|&(name, angle_deg, indices, mast)| scale_tier(name, angle_deg, indices, mast))
        .collect()
}

// The four newer shapes. Every table below was fitted (and its 4-decimal masts
// re-checked) so that, on the template's default preform and after a remap to each
// gear it offers: every facet reaches the surface, the smallest facet is well above
// the manufacturability threshold, no preform plane cuts the stone, the girdle is
// alive and the outline has the intended length-to-width ratio. Edit a mast by hand
// and the per-template tests in this file tell you what broke.

/// Oval, length-to-width 1.35, 2-fold with mirror. Girdle masts follow the ellipse
/// `hypot(cos a, 1.35 sin a)` around the 16 girdle positions; crown 35 / pavilion 41
/// degrees, the corundum-like figures. Stored in cutting order: the pavilion mains, the
/// girdle, the lower girdle and the culet, then the crown mains, the stars and the upper
/// girdle, the table last.
const OVAL_ROWS: &[Row] = &[
    ("Pavilion Main 1", -41.0, &[0.0, 48.0], 0.6787),
    ("Pavilion Main 2", -41.0, &[12.0, 36.0, 60.0, 84.0], 0.802),
    ("Pavilion Main 3", -41.0, &[24.0, 72.0], 0.9083),
    ("Girdle 1", 90.0, &[0.0, 48.0], 1.0),
    ("Girdle 2", 90.0, &[6.0, 42.0, 54.0, 90.0], 1.0585),
    ("Girdle 3", 90.0, &[12.0, 36.0, 60.0, 84.0], 1.188),
    ("Girdle 4", 90.0, &[18.0, 30.0, 66.0, 78.0], 1.3046),
    ("Girdle 5", 90.0, &[24.0, 72.0], 1.35),
    ("Lower Girdle 1", -42.0, &[1.0, 47.0, 49.0, 95.0], 0.695),
    ("Lower Girdle 2", -42.0, &[11.0, 37.0, 59.0, 85.0], 0.7905),
    ("Lower Girdle 3", -42.0, &[13.0, 35.0, 61.0, 83.0], 0.8405),
    ("Lower Girdle 4", -42.0, &[23.0, 25.0, 71.0, 73.0], 0.9237),
    ("Culet", -0.0, &[], 0.8705),
    ("Crown Main 1", 35.0, &[0.0, 48.0], 0.5982),
    ("Crown Main 2", 35.0, &[12.0, 36.0, 60.0, 84.0], 0.706),
    ("Crown Main 3", 35.0, &[24.0, 72.0], 0.7989),
    ("Star 1", 18.0, &[6.0, 42.0, 54.0, 90.0], 0.4812),
    ("Star 2", 18.0, &[18.0, 30.0, 66.0, 78.0], 0.5797),
    ("Upper Girdle 1", 41.0, &[1.0, 47.0, 49.0, 95.0], 0.6423),
    ("Upper Girdle 2", 41.0, &[11.0, 37.0, 59.0, 85.0], 0.7528),
    ("Upper Girdle 3", 41.0, &[13.0, 35.0, 61.0, 83.0], 0.7953),
    ("Upper Girdle 4", 41.0, &[23.0, 25.0, 71.0, 73.0], 0.8814),
    ("Table", 0.0, &[], 0.34),
];

/// Cushion: a rounded square (girdle masts `0.5 (|cos a| + |sin a|) + 0.5`), 4-fold
/// with mirror, crown 38 / pavilion 43 degrees, the quartz-like figures. Stored in
/// cutting order: the pavilion mains, the girdle, the lower girdle and the culet, then the
/// crown mains, the star and the upper girdle, the table last.
const CUSHION_ROWS: &[Row] = &[
    ("Pavilion Main 1", -43.0, &[0.0, 24.0, 48.0, 72.0], 0.7039),
    ("Pavilion Main 2", -43.0, &[12.0, 36.0, 60.0, 84.0], 0.8452),
    ("Girdle 1", 90.0, &[0.0, 24.0, 48.0, 72.0], 1.0),
    (
        "Girdle 2",
        90.0,
        &[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0],
        1.1533,
    ),
    ("Girdle 3", 90.0, &[12.0, 36.0, 60.0, 84.0], 1.2071),
    (
        "Lower Girdle 1",
        -44.0,
        &[1.0, 23.0, 25.0, 47.0, 49.0, 71.0, 73.0, 95.0],
        0.7148,
    ),
    (
        "Lower Girdle 2",
        -44.0,
        &[11.0, 13.0, 35.0, 37.0, 59.0, 61.0, 83.0, 85.0],
        0.8583,
    ),
    ("Culet", -0.0, &[], 0.9357),
    ("Crown Main 1", 38.0, &[0.0, 24.0, 48.0, 72.0], 0.6393),
    ("Crown Main 2", 38.0, &[12.0, 36.0, 60.0, 84.0], 0.7668),
    (
        "Star",
        20.0,
        &[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0],
        0.5165,
    ),
    (
        "Upper Girdle 1",
        42.0,
        &[1.0, 23.0, 25.0, 47.0, 49.0, 71.0, 73.0, 95.0],
        0.7,
    ),
    (
        "Upper Girdle 2",
        42.0,
        &[11.0, 13.0, 35.0, 37.0, 59.0, 61.0, 83.0, 85.0],
        0.8283,
    ),
    ("Table", 0.0, &[], 0.33),
];

/// Emerald step cut, length-to-width 1.4, 2-fold with mirror: three crown steps
/// (34 / 38 / 42 degrees towards the girdle) and three pavilion steps (42 / 45 / 48
/// degrees towards the girdle) on the long sides, the short ends and the cut
/// corners, meeting in a keel. Beryl-like figures.
const EMERALD_ROWS: &[Row] = &[
    ("Table", 0.0, &[], 0.3292),
    ("Crown Side 1", 42.0, &[0.0, 48.0], 0.6914),
    ("Crown Side 2", 38.0, &[0.0, 48.0], 0.6512),
    ("Crown Side 3", 34.0, &[0.0, 48.0], 0.6191),
    ("Crown End 1", 42.0, &[24.0, 72.0], 0.9591),
    ("Crown End 2", 38.0, &[24.0, 72.0], 0.8975),
    ("Crown End 3", 34.0, &[24.0, 72.0], 0.8428),
    ("Crown Corner 1", 42.0, &[12.0, 36.0, 60.0, 84.0], 0.997),
    ("Crown Corner 2", 38.0, &[12.0, 36.0, 60.0, 84.0], 0.9324),
    ("Crown Corner 3", 34.0, &[12.0, 36.0, 60.0, 84.0], 0.8744),
    ("Girdle Side", 90.0, &[0.0, 48.0], 1.0),
    ("Girdle End", 90.0, &[24.0, 72.0], 1.4),
    ("Girdle Corner", 90.0, &[12.0, 36.0, 60.0, 84.0], 1.4566),
    ("Pavilion Side 1", -48.0, &[0.0, 48.0], 0.7632),
    ("Pavilion Side 2", -45.0, &[0.0, 48.0], 0.7518),
    ("Pavilion Side 3", -42.0, &[0.0, 48.0], 0.7627),
    ("Pavilion End 1", -48.0, &[24.0, 72.0], 1.0605),
    ("Pavilion End 2", -45.0, &[24.0, 72.0], 1.0346),
    ("Pavilion End 3", -42.0, &[24.0, 72.0], 1.0304),
    (
        "Pavilion Corner 1",
        -48.0,
        &[12.0, 36.0, 60.0, 84.0],
        1.1026,
    ),
    (
        "Pavilion Corner 2",
        -45.0,
        &[12.0, 36.0, 60.0, 84.0],
        1.0535,
    ),
    (
        "Pavilion Corner 3",
        -42.0,
        &[12.0, 36.0, 60.0, 84.0],
        1.0261,
    ),
];

/// Princess: a square with clipped corners (girdle masts 1.0 on the sides, 1.3 on
/// the diagonals), 4-fold with mirror, crown 35 / pavilion 41 degrees, the
/// corundum-like figures. A modified brilliant: the pavilion has side and corner
/// mains, not chevrons. Stored in cutting order: the pavilion mains, the girdle, the lower
/// girdle and the culet, then the crown mains, the star and the upper girdle, the table
/// last.
const PRINCESS_ROWS: &[Row] = &[
    ("Pavilion Main 1", -41.0, &[0.0, 24.0, 48.0, 72.0], 0.6787),
    ("Pavilion Main 2", -41.0, &[12.0, 36.0, 60.0, 84.0], 0.8755),
    ("Girdle 1", 90.0, &[0.0, 24.0, 48.0, 72.0], 1.0),
    ("Girdle 2", 90.0, &[12.0, 36.0, 60.0, 84.0], 1.3),
    (
        "Lower Girdle 1",
        -42.0,
        &[1.0, 23.0, 25.0, 47.0, 49.0, 71.0, 73.0, 95.0],
        0.7225,
    ),
    (
        "Lower Girdle 2",
        -42.0,
        &[11.0, 13.0, 35.0, 37.0, 59.0, 61.0, 83.0, 85.0],
        0.8878,
    ),
    ("Culet", -0.0, &[], 0.8705),
    ("Crown Main 1", 35.0, &[0.0, 24.0, 48.0, 72.0], 0.5982),
    ("Crown Main 2", 35.0, &[12.0, 36.0, 60.0, 84.0], 0.7702),
    (
        "Star",
        18.0,
        &[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0],
        0.4927,
    ),
    (
        "Upper Girdle 1",
        41.0,
        &[1.0, 23.0, 25.0, 47.0, 49.0, 71.0, 73.0, 95.0],
        0.7048,
    ),
    (
        "Upper Girdle 2",
        41.0,
        &[11.0, 13.0, 35.0, 37.0, 59.0, 61.0, 83.0, 85.0],
        0.8537,
    ),
    ("Table", 0.0, &[], 0.33),
];

fn oval_tiers() -> Vec<ConstraintTier> {
    tiers_from_rows(OVAL_ROWS)
}

fn cushion_tiers() -> Vec<ConstraintTier> {
    tiers_from_rows(CUSHION_ROWS)
}

fn emerald_tiers() -> Vec<ConstraintTier> {
    tiers_from_rows(EMERALD_ROWS)
}

fn princess_tiers() -> Vec<ConstraintTier> {
    tiers_from_rows(PRINCESS_ROWS)
}

/// `n_D` of the built-in Diamond (2.41726) rounded to the figure templates are
/// designed for; every figure below is checked against the catalogue by a test.
const RI_DIAMOND: f64 = 2.417;
/// Sapphire (corundum), `n_D` 1.76808.
const RI_SAPPHIRE: f64 = 1.768;
/// Quartz, `n_D` 1.54421 -- also the 1.54 the teaching designs always assumed.
const RI_QUARTZ: f64 = 1.544;
/// Emerald (beryl), `n_D` 1.5791.
const RI_BERYL: f64 = 1.579;

/// The preform every template is cut from: a 96-sided cylinder of half-width 1.5, `l_over_w`
/// long, and tall enough (2.2) that no finished stone touches the rough's own bottom.
const fn rough(l_over_w: f64) -> PreformSpec {
    PreformSpec::cylinder(96, 1.5, l_over_w, 2.2)
}

/// The built-in template gallery.
///
/// **Only append.** The position in this table is the template's identity for the web
/// app, the worked-example guide and saved fixtures: `EditorSession::from_template`'s
/// index `n >= 1` is `TEMPLATES[n - 1]`, with `0` reserved for "Empty". The display
/// order of the New Design dialog (featured shapes first) is built from
/// [`TemplateSpec::featured`] and does not depend on this order.
pub const TEMPLATES: &[TemplateSpec] = &[
    TemplateSpec {
        name: "Standard Round Brilliant",
        shape: "8-fold round brilliant",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "The classic eight-main round brilliant: table, star, crown main, upper girdle, girdle, pavilion main, lower girdle, culet.",
        family: ShapeFamily::Round,
        featured: true,
        design_ri: RI_DIAMOND,
        default_material: "Diamond",
        preform: rough(1.0),
        build: round_brilliant_tiers,
    },
    TemplateSpec {
        name: "Round Brilliant -- Shallow Pavilion",
        shape: "8-fold round brilliant",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "The standard round brilliant with a shallower pavilion main -- more windowing risk, a teaching example for the critical-angle margin.",
        family: ShapeFamily::Round,
        featured: false,
        design_ri: RI_QUARTZ,
        default_material: "Quartz",
        preform: rough(1.0),
        build: shallow_pavilion_tiers,
    },
    TemplateSpec {
        name: "Round Brilliant -- Deep Pavilion",
        shape: "8-fold round brilliant",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "The standard round brilliant with a deeper pavilion main, closer to a typical critical angle -- a teaching example for the Marginal/Windows risk bands.",
        family: ShapeFamily::Round,
        featured: false,
        design_ri: RI_QUARTZ,
        default_material: "Quartz",
        preform: rough(1.0),
        build: deep_pavilion_tiers,
    },
    TemplateSpec {
        name: "Simple Teaching Design",
        shape: "8-fold, 3 tiers",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "A table, a girdle band and one ring of pavilion facets -- the smallest design that solves and closes, for a first look at the tier table.",
        family: ShapeFamily::Round,
        featured: false,
        design_ri: RI_QUARTZ,
        default_material: "Quartz",
        preform: rough(1.0),
        build: simple_teaching_tiers,
    },
    TemplateSpec {
        name: "Rich Teaching Design",
        shape: "8-fold, 4 tiers",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "A table, a girdle band, one crown ring and one pavilion ring -- one step past the Simple Teaching Design, with a real crown to edit.",
        family: ShapeFamily::Round,
        featured: false,
        design_ri: RI_QUARTZ,
        default_material: "Quartz",
        preform: rough(1.0),
        build: rich_teaching_tiers,
    },
    TemplateSpec {
        name: "Oval Brilliant",
        shape: "2-fold oval, L/W 1.35",
        gear_teeth: 96,
        symmetry_order: 2,
        mirror: true,
        description: "An elongated brilliant with a length-to-width ratio of 1.35, cut for corundum-like stones (sapphire, ruby): crown 35, pavilion 41 degrees.",
        family: ShapeFamily::Oval,
        featured: true,
        design_ri: RI_SAPPHIRE,
        default_material: "Sapphire",
        preform: rough(1.35),
        build: oval_tiers,
    },
    TemplateSpec {
        name: "Cushion Brilliant",
        shape: "4-fold cushion",
        gear_teeth: 96,
        symmetry_order: 4,
        mirror: true,
        description: "A rounded-square brilliant with a deeper pavilion, cut for quartz-like stones (quartz, citrine, amethyst): crown 38, pavilion 43 degrees.",
        family: ShapeFamily::Cushion,
        featured: true,
        design_ri: RI_QUARTZ,
        default_material: "Quartz",
        preform: rough(1.0),
        build: cushion_tiers,
    },
    TemplateSpec {
        name: "Emerald Step Cut",
        shape: "2-fold step cut, L/W 1.4",
        gear_teeth: 96,
        symmetry_order: 2,
        mirror: true,
        description: "A rectangular step cut with cut corners and a length-to-width ratio of 1.4: three crown steps and three pavilion steps, cut for beryl (emerald, aquamarine).",
        family: ShapeFamily::Emerald,
        featured: true,
        design_ri: RI_BERYL,
        default_material: "Emerald",
        preform: rough(1.4),
        build: emerald_tiers,
    },
    TemplateSpec {
        name: "Princess Cut",
        shape: "4-fold square brilliant",
        gear_teeth: 96,
        symmetry_order: 4,
        mirror: true,
        description: "A square modified brilliant with clipped corners, cut for corundum-like stones: crown 35, pavilion 41 degrees, mains on the sides and the corners.",
        family: ShapeFamily::Princess,
        featured: true,
        design_ri: RI_SAPPHIRE,
        default_material: "Sapphire",
        preform: rough(1.0),
        build: princess_tiers,
    },
];

impl Design {
    /// The concave-facet fixture later work packages pin their numbers against:
    /// a five-tier 16-tooth round brilliant on a 2 x 1 x 2 block (the same
    /// schedule the cut-core edit round-trip tests use), plus two concave tiers,
    ///
    /// - `Groove`: `CYL`, φ = −42, the eight even indices, θ = 0,
    ///   X = 0, Y = 0.15, Z = 0.03, D = 0.25, reciprocating;
    /// - `Dimple`: `SPH`, φ = +36, indices 0/4/8/12, X = Y = 0, Z = 0.02,
    ///   D = 0.1, plunge (a crown-side dimple).
    ///
    /// **Frozen once published**: tests in other crates pin geometry, hashes and
    /// text against these exact numbers, so change them only by adding a second
    /// fixture. Not a template for users, and the concave tiers carry the
    /// provisional `v0` frame conventions (plan §11a).
    ///
    /// # Panics
    ///
    /// Never in practice: the embedded `.asc` text is a constant that parses.
    #[must_use]
    pub fn concave_fixture() -> Self {
        const ASC: &str = "GemCad 5.0\n\
             g 16 0.0\n\
             y 4 n\n\
             I 1.54\n\
             a 90.000000 1.00000000 0 4 8 12 G Set girdle thickness\n\
             a -42.000000 0.60000000 0 4 8 12 G Set stone size\n\
             a -38.000000 0.55000000 2 6 10 14 G Set stone size\n\
             a 32.000000 0.45000000 0 4 8 12 G Set stone size\n\
             a 40.000000 0.40000000 2 6 10 14 G Set stone size\n";
        let schedule =
            indicatrix_formats::asc::parse_asc(ASC).expect("the fixture's .asc text must parse");
        let mut design = Self::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
        design.girdle_diameter_mm = Some(6.5);
        design.material = MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: Some(1.54),
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        };
        design.concave_tiers = vec![
            ConcaveTier {
                name: "Groove".to_owned(),
                angle_deg: -42.0,
                indices: (0..8).map(|i| f64::from(2 * i)).collect(),
                instructions: String::new(),
                tool: ConcaveTool::Cylinder,
                tool_azimuth_deg: 0.0,
                displacement: [0.0, 0.15, 0.03],
                diameter_ratio: 0.25,
                tool_angle_deg: None,
                motion: ToolMotion::Reciprocating,
            },
            ConcaveTier {
                name: "Dimple".to_owned(),
                angle_deg: 36.0,
                indices: vec![0.0, 4.0, 8.0, 12.0],
                instructions: String::new(),
                tool: ConcaveTool::Sphere,
                tool_azimuth_deg: 0.0,
                displacement: [0.0, 0.0, 0.02],
                diameter_ratio: 0.1,
                tool_angle_deg: None,
                motion: ToolMotion::Plunge,
            },
        ];
        design.ensure_concave_tier_ids();
        design
    }
}

#[cfg(test)]
mod tests;
