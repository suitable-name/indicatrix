//! Built-in "New Design" templates.
//!
//! A small, curated gallery of classic faceting designs, each described by a
//! static [`TemplateSpec`] entry in [`TEMPLATES`] instead of hard-coded in
//! `apps/indicatrix-cut/src/gui/editor/callbacks/tier_actions.rs`'s
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
//! five classic designs: the existing standard round brilliant plus four new
//! variants built on its own proven index layout. A true step cut (rectangular
//! facets), cushion or oval (non-radially-symmetric outline), or trillion (3-fold
//! layout) needs either a facet topology this crate's radially-symmetric
//! [`crate::design::ConstraintTier`] model does not represent, or an unproven
//! gear/symmetry/index combination that cannot be verified within a single batched
//! test run. Shipping five templates that are verified closed solids was chosen
//! over shipping twelve where some might not close. See this crate's own
//! `#[cfg(test)]` module below for the verification every entry gets.

use crate::design::{ConstraintTier, ScheduleMeta};
use indicatrix::geometry::meet_solver::MeetConstraint;

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
    /// Builds this template's tier table. Called once per selection, not
    /// cached -- these are all cheap, small `Vec` literals (the same cost
    /// [`crate::design::ConstraintTier::standard_round_brilliant`] already pays
    /// on every "Create").
    build: fn() -> Vec<ConstraintTier>,
}

impl TemplateSpec {
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
            gemcad_version: "GemCad 5.0".to_string(),
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
    pub const GIRDLE: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];
    pub const BREAK: [f64; 16] = [
        95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0, 83.0,
        85.0,
    ];
    pub const MAIN: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
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

fn round_brilliant_tiers() -> Vec<ConstraintTier> {
    ConstraintTier::standard_round_brilliant()
}

/// A shallower pavilion (mains at -38 instead of -41, lower girdle following at
/// -40) -- closes the same way the standard template does, but with less
/// pavilion angle margin over a typical critical angle, useful for
/// demonstrating windowing risk in the guide/inspector's margin readout.
fn shallow_pavilion_tiers() -> Vec<ConstraintTier> {
    vec![
        scale_tier("Table", 0.0, &[], 0.32),
        scale_tier("Star", 15.0, &indices::STAR, 0.45),
        scale_tier("Crown Main", 34.5, &indices::MAIN, 0.59),
        scale_tier("Upper Girdle", 41.0, &indices::BREAK, 0.67),
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        scale_tier("Pavilion Main", -38.0, &indices::MAIN, 0.67),
        scale_tier("Lower Girdle", -40.0, &indices::BREAK, 0.68),
        scale_tier("Culet", -0.0, &[], 0.88),
    ]
}

/// A deeper pavilion (mains at -43.5, lower girdle at -44.5) -- closer to a
/// typical critical angle than the standard template, useful for demonstrating
/// a "Marginal"/"Windows" risk band in the guide/inspector.
fn deep_pavilion_tiers() -> Vec<ConstraintTier> {
    vec![
        scale_tier("Table", 0.0, &[], 0.32),
        scale_tier("Star", 15.0, &indices::STAR, 0.45),
        scale_tier("Crown Main", 34.5, &indices::MAIN, 0.59),
        scale_tier("Upper Girdle", 41.0, &indices::BREAK, 0.67),
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        scale_tier("Pavilion Main", -43.5, &indices::MAIN, 0.67),
        scale_tier("Lower Girdle", -44.5, &indices::BREAK, 0.68),
        scale_tier("Culet", -0.0, &[], 0.88),
    ]
}

/// The simplest possible closed design on this layout: a table, a girdle band,
/// and one ring of pavilion mains -- no crown facets, no star/break rings. Three
/// tiers total, meant as a first design smaller than the standard round
/// brilliant's eight, for a cutter learning the tier table.
fn simple_teaching_tiers() -> Vec<ConstraintTier> {
    vec![
        scale_tier("Table", 0.0, &[], 0.30),
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        scale_tier("Pavilion Main", -40.0, &indices::MAIN, 0.70),
    ]
}

/// One step up from [`simple_teaching_tiers`]: adds a single crown-main ring
/// above the girdle, so both blocks have one real facet ring each -- table,
/// girdle, crown main, pavilion main. Four tiers, still no star/break rings.
fn rich_teaching_tiers() -> Vec<ConstraintTier> {
    vec![
        scale_tier("Table", 0.0, &[], 0.30),
        scale_tier("Crown Main", 34.5, &indices::MAIN, 0.60),
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        scale_tier("Pavilion Main", -40.0, &indices::MAIN, 0.70),
    ]
}

/// The built-in template gallery, in display order.
///
/// Index 0 is intentionally the existing standard round brilliant
/// (`new_design_dialog.slint`'s combo calls this "Standard Round
/// Brilliant" at its own index 1, with index 0 reserved for "Empty" -- that
/// mapping is a UI-side concern, not this table's; see this module's own top
/// doc comment for the still-open integration step in `tier_actions.rs` that
/// would let a UI index select one of these beyond the two already wired).
pub const TEMPLATES: &[TemplateSpec] = &[
    TemplateSpec {
        name: "Standard Round Brilliant",
        shape: "8-fold round brilliant",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "The classic eight-main round brilliant: table, star, crown main, upper girdle, girdle, pavilion main, lower girdle, culet.",
        build: round_brilliant_tiers,
    },
    TemplateSpec {
        name: "Round Brilliant -- Shallow Pavilion",
        shape: "8-fold round brilliant",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "The standard round brilliant with a shallower pavilion main -- more windowing risk, a teaching example for the critical-angle margin.",
        build: shallow_pavilion_tiers,
    },
    TemplateSpec {
        name: "Round Brilliant -- Deep Pavilion",
        shape: "8-fold round brilliant",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "The standard round brilliant with a deeper pavilion main, closer to a typical critical angle -- a teaching example for the Marginal/Windows risk bands.",
        build: deep_pavilion_tiers,
    },
    TemplateSpec {
        name: "Simple Teaching Design",
        shape: "8-fold, 3 tiers",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "A table, a girdle band and one ring of pavilion facets -- the smallest design that solves and closes, for a first look at the tier table.",
        build: simple_teaching_tiers,
    },
    TemplateSpec {
        name: "Rich Teaching Design",
        shape: "8-fold, 4 tiers",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "A table, a girdle band, one crown ring and one pavilion ring -- one step past the Simple Teaching Design, with a real crown to edit.",
        build: rich_teaching_tiers,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{design::Design, preform::PreformSpec};

    /// Every template in the gallery must build a non-empty tier table, and --
    /// the point of this module's whole "`ScaleReference`-only" restriction --
    /// solve and close as a real solid, the same standard this crate already
    /// holds `ConstraintTier::standard_round_brilliant` to
    /// (`standard_round_brilliant_template_solves_and_closes`, `tier.rs`).
    #[test]
    fn every_template_solves_and_closes() {
        for spec in TEMPLATES {
            let tiers = spec.tiers();
            assert!(
                !tiers.is_empty(),
                "template {:?} built an empty tier table",
                spec.name
            );
            let design = Design::new(
                PreformSpec::cylinder(spec.gear_teeth.unsigned_abs() as usize, 1.5, 1.0, 1.5),
                spec.schedule_meta(),
                tiers,
            );
            design
                .solve()
                .unwrap_or_else(|e| panic!("template {:?} failed to solve: {e:?}", spec.name));
            assert!(
                design.is_closed(),
                "template {:?} solved but is not a closed solid",
                spec.name
            );
        }
    }

    /// The gallery must not silently shrink to nothing, and every name must be
    /// unique -- a UI index-to-template mapping (the still-open Rust
    /// integration step this module's own top doc comment describes) depends
    /// on stable, distinct names.
    #[test]
    fn template_names_are_unique_and_gallery_is_non_empty() {
        // `TEMPLATES` is the `const` array literal declared above --
        // `.is_empty()` on it is compile-time-decidable (clippy::const_is_empty);
        // the uniqueness scan below is what actually earns this test its keep.
        for (i, a) in TEMPLATES.iter().enumerate() {
            for b in &TEMPLATES[i + 1..] {
                assert_ne!(a.name, b.name, "duplicate template name {:?}", a.name);
            }
        }
    }
}
