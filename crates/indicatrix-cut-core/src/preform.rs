//! The preform: the physical rough a design's facets are cut from.
//!
//! **This is a different concept from `indicatrix::geometry::stone_metrics::BLANK_HALF_EXTENT`.**
//! That constant is six axis-aligned planes at an absurd half-extent, used only to
//! let `measure_solid`/`build_solid_mesh` *detect* an unbounded arrangement; this
//! module does not touch it.
//!
//! [`PreformSpec`] instead models a *design input*: the small, finite piece of rough
//! the lapidary actually starts cutting from -- a cylinder (round or oval) or a
//! rectangular block, sized to roughly the finished stone's own proportions.
//! [`PreformSpec::planes`] alone (before a single facet is added) is already a
//! closed, finite, renderable solid, so a brand-new design shows something real
//! from the first frame, and a facet cutting outside the rough becomes a visible
//! physical fact instead of an abstract validation error.

use glam::DVec3;

/// The rough's cross-sectional shape, viewed down the vertical (`y`) axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreformShape {
    /// A rectangular block: `2 * half_width` by `2 * half_length` in cross
    /// section (a plain 4-sided prism -- see [`PreformSpec::planes`]).
    Block,
    /// A cylinder (round when `length_over_width == 1.0`, otherwise an oval),
    /// approximated by an `sides`-sided prism whose faces are tangent to the true
    /// ellipse, generalizing the "circumscribed polygon" construction
    /// `StandardGemCuts::standard_round_brilliant` uses for its 16-facet girdle wall.
    ///
    /// `sides` should be at least 3; [`PreformSpec::planes`] clamps a smaller value
    /// up rather than producing a degenerate/unbounded arrangement, since an editor
    /// calling this mid-keystroke (e.g. a "fold count" field being typed) should
    /// never see a crash or an empty viewport.
    Cylinder { sides: usize },
}

/// A design's uncut rough, as a small closed plane set.
///
/// `half_width`/`length_over_width`/`depth` are in the same "mast unit" scale as every
/// other plane offset in this codebase (a design's facets are masts of order 1;
/// `crate::design::Design` hands `PreformSpec::planes`'s output to
/// `indicatrix::geometry::stone_metrics::build_solid_mesh` in exactly that convention, no
/// rescaling).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreformSpec {
    pub shape: PreformShape,
    /// Half the rough's width (its shorter horizontal extent, along `x`).
    pub half_width: f64,
    /// Length-over-width ratio; the rough's half-length (along `z`) is
    /// `half_width * length_over_width`.
    pub length_over_width: f64,
    /// Total top-to-bottom extent (table-to-culet direction); the rough
    /// spans `y` in `[-depth/2, +depth/2]`.
    pub depth: f64,
}

impl PreformSpec {
    /// A rectangular block preform.
    #[must_use]
    pub const fn block(half_width: f64, length_over_width: f64, depth: f64) -> Self {
        Self {
            shape: PreformShape::Block,
            half_width,
            length_over_width,
            depth,
        }
    }

    /// A cylindrical (or, when `length_over_width != 1.0`, oval) preform with
    /// an explicit polygon side count.
    #[must_use]
    pub const fn cylinder(
        sides: usize,
        half_width: f64,
        length_over_width: f64,
        depth: f64,
    ) -> Self {
        Self {
            shape: PreformShape::Cylinder { sides },
            half_width,
            length_over_width,
            depth,
        }
    }

    /// A cylindrical preform whose polygon side count matches `schedule`'s
    /// own index-gear tooth count -- the natural "fold count" for a rough
    /// that will be faceted on that same index gear, and fine enough
    /// (typically 96 sides) to read as a smooth round in the viewport
    /// without the visible faceting a hand-picked low side count would show.
    #[must_use]
    pub const fn cylinder_for_schedule(
        schedule: &indicatrix_formats::asc::AscSchedule,
        half_width: f64,
        length_over_width: f64,
        depth: f64,
    ) -> Self {
        Self::cylinder(
            schedule.gear_teeth_abs() as usize,
            half_width,
            length_over_width,
            depth,
        )
    }

    /// The preform's own closed half-space plane set, in the `n . x <= m`
    /// convention [`indicatrix::geometry::stone_metrics::measure_solid`] and
    /// [`indicatrix::geometry::stone_metrics::build_solid_mesh`] take.
    ///
    /// Always a valid, finite, closed solid on its own (at least a
    /// triangular prism, 5 planes) -- this is the property
    /// [`crate::design::Design`] relies on to guarantee a fresh design (no
    /// facets yet) already renders as a real stone.
    #[must_use]
    pub fn planes(&self) -> Vec<(DVec3, f64)> {
        let half_length = self.half_width * self.length_over_width;
        let half_depth = self.depth * 0.5;
        let mut planes = match self.shape {
            PreformShape::Block => vec![
                (DVec3::X, self.half_width),
                (DVec3::NEG_X, self.half_width),
                (DVec3::Z, half_length),
                (DVec3::NEG_Z, half_length),
            ],
            PreformShape::Cylinder { sides } => {
                let sides = sides.max(3);
                let mut walls = Vec::with_capacity(sides);
                for i in 0..sides {
                    let phi = std::f64::consts::TAU * (i as f64) / (sides as f64);
                    let (sin_phi, cos_phi) = phi.sin_cos();
                    let n = DVec3::new(cos_phi, 0.0, sin_phi);
                    // Support-function offset of the ellipse in direction `n`: the
                    // plane `n . x <= m` is tangent to (never cuts into) the true
                    // ellipse, so the resulting prism circumscribes it.
                    let m = (self.half_width * cos_phi).hypot(half_length * sin_phi);
                    walls.push((n, m));
                }
                walls
            }
        };
        planes.push((DVec3::Y, half_depth));
        planes.push((DVec3::NEG_Y, half_depth));
        planes
    }

    /// [`Self::planes`], but with the rough's vertical (`y`) span shifted by
    /// `y_offset` instead of always centred at the origin.
    ///
    /// A real piece of rough is rarely centred on the girdle a cutter plans to
    /// cut into it -- "the girdle should sit 2 mm below the top of this
    /// stone" is an offset, not a dimension [`PreformSpec`]'s own fields carry
    /// (see the module docs: every existing plane always spans
    /// `[-depth/2, +depth/2]`). This is the pure geometry half of that:
    /// `y_offset > 0.0` raises the whole rough (moves both the `+Y` and `-Y`
    /// planes up), `y_offset < 0.0` lowers it, in the same mast-unit
    /// convention [`Self::planes`] uses.
    ///
    /// [`PreformSpec`] itself carries no offset field -- adding one would be a
    /// breaking change to every existing struct-literal construction site
    /// (`crates/indicatrix-cut-core/src/native/convert.rs`'s
    /// `preform_spec_from_table`, in particular, builds a full literal with
    /// no `..Default::default()`). A caller that wants a *persistent*
    /// per-design offset should store it alongside `girdle_diameter_mm` on
    /// [`crate::design::Design`] instead
    /// and pass it through here at every [`Self::planes`] call site
    /// (`crate::design::export`'s `planes`/`planes_from_solved`) rather than
    /// widening this struct.
    #[must_use]
    pub fn planes_offset(&self, y_offset: f64) -> Vec<(DVec3, f64)> {
        let half_depth = self.depth * 0.5;
        let mut planes = self.planes();
        // `Self::planes` always pushes the `+Y` plane then the `-Y` plane
        // last, regardless of `shape` -- see its own body. Indexing from the
        // end avoids a float-equality comparison against `DVec3::Y`/`NEG_Y`
        // to find them.
        let len = planes.len();
        planes[len - 2].1 = half_depth + y_offset;
        planes[len - 1].1 = half_depth - y_offset;
        planes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::stone_metrics::{SolidStatus, build_solid_mesh, measure_solid};

    /// A block preform alone must already be a finite, positive-volume,
    /// closed solid -- exactly `2*half_width x depth x 2*length` in extent.
    #[test]
    fn block_preform_alone_is_closed() {
        let preform = PreformSpec::block(1.0, 1.5, 0.8);
        let planes = preform.planes();
        let metrics = measure_solid(&planes).expect("block preform must be a closed solid");
        let expected_volume = 2.0_f64 * 3.0 * 0.8; // (2*half_width) * (2*half_length) * depth
        assert!(
            (metrics.volume - expected_volume).abs() < 1e-9,
            "{}",
            metrics.volume
        );
        assert!((metrics.width_axis - 2.0).abs() < 1e-9);
        assert!((metrics.length_axis - 3.0).abs() < 1e-9);
        assert!((metrics.total_height - 0.8).abs() < 1e-9);
        assert!(matches!(build_solid_mesh(&planes), SolidStatus::Closed(_)));
    }

    /// A round cylinder preform (`length_over_width == 1.0`) alone must be
    /// closed, with every wall plane at exactly `half_width` from the axis
    /// (the round-cross-section special case of the support-function
    /// formula) and a volume approaching `pi * r^2 * depth` as the side
    /// count grows.
    #[test]
    fn round_cylinder_preform_alone_is_closed_and_approaches_the_true_cylinder_volume() {
        let preform = PreformSpec::cylinder(96, 1.0, 1.0, 0.8);
        let planes = preform.planes();
        for &(_, m) in &planes[..96] {
            assert!((m - 1.0).abs() < 1e-12, "wall offset {m} != half_width");
        }
        let metrics = measure_solid(&planes).expect("cylinder preform must be a closed solid");
        let true_volume = std::f64::consts::PI * 1.0 * 1.0 * 0.8;
        // A circumscribed n-gon has strictly more area than the true circle,
        // converging as `sides` grows -- not a bit-exact match.
        assert!(metrics.volume > true_volume);
        assert!((metrics.volume - true_volume).abs() < 0.01 * true_volume);
        assert!(matches!(build_solid_mesh(&planes), SolidStatus::Closed(_)));
    }

    /// An oval cylinder preform (`length_over_width != 1.0`) must still
    /// close, and its axis-aligned extents must match the requested
    /// half-width/half-length (the tangent planes at `phi = 0` and
    /// `phi = pi/2` sit exactly at those extents by construction).
    #[test]
    fn oval_cylinder_preform_is_closed_with_the_requested_extents() {
        let preform = PreformSpec::cylinder(64, 1.0, 1.5, 0.8);
        let planes = preform.planes();
        let metrics = measure_solid(&planes).expect("oval preform must be a closed solid");
        assert!(
            (metrics.width_axis - 2.0).abs() < 1e-9,
            "{}",
            metrics.width_axis
        );
        assert!(
            (metrics.length_axis - 3.0).abs() < 1e-9,
            "{}",
            metrics.length_axis
        );
    }

    /// `planes_offset` must shift the rough's vertical span without changing
    /// its total depth or horizontal extents, and `y_offset == 0.0` must
    /// match `planes` exactly.
    #[test]
    fn planes_offset_shifts_the_vertical_span_only() {
        let preform = PreformSpec::block(1.0, 1.5, 0.8);
        assert_eq!(preform.planes_offset(0.0), preform.planes());

        let shifted = preform.planes_offset(0.3);
        let metrics = measure_solid(&shifted).expect("shifted block must still close");
        assert!((metrics.total_height - 0.8).abs() < 1e-9);
        assert!((metrics.width_axis - 2.0).abs() < 1e-9);
        assert!((metrics.length_axis - 3.0).abs() < 1e-9);

        // Shifting up by 0.3 must move both the top and bottom by 0.3.
        let unshifted_top = measure_solid(&preform.planes()).unwrap().total_height / 2.0;
        assert!((shifted[shifted.len() - 2].1 - (unshifted_top + 0.3)).abs() < 1e-9);
        assert!((shifted[shifted.len() - 1].1 - (unshifted_top - 0.3)).abs() < 1e-9);
    }

    /// A degenerate side count (0, 1, 2) must be clamped up to a valid
    /// triangular prism rather than producing an unbounded or panicking
    /// result -- an editor mid-keystroke on a "sides" field is exactly this
    /// case.
    #[test]
    fn degenerate_side_counts_are_clamped_to_a_valid_prism() {
        for sides in [0usize, 1, 2] {
            let preform = PreformSpec::cylinder(sides, 1.0, 1.0, 0.8);
            assert!(
                matches!(build_solid_mesh(&preform.planes()), SolidStatus::Closed(_)),
                "sides={sides} must still close"
            );
        }
    }
}
