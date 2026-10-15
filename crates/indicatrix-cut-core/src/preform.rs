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
//!
//! A third shape, [`PreformShape::Planes`], carries an arbitrary convex plane set
//! ([`PlaneSet`]) in the design's own frame. It exists for the Rough Planner, which
//! hands the editor "the piece of rough this stone comes from" (the sawn piece box,
//! clipped by the rough's outline and moved into the design's frame) rather than a
//! prism sized to the finished stone. It is persisted as its planes (the native file
//! formats store the half-spaces and the source note), and [`PreformSpec::planes`]
//! returns it unchanged.

use glam::DVec3;
use indicatrix::geometry::stone_metrics::measure_solid;
use std::sync::Arc;

/// A convex plane set that is a preform of its own.
///
/// It is the piece of rough a planned stone comes from (the Rough Planner's sawn piece box,
/// clipped by the rough's outline and moved into the design's frame), or any other rough a
/// caller has as half-spaces.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaneSet {
    /// The half-spaces `n . x <= m` in model units, in the design's own frame (the same
    /// convention as [`PreformSpec::planes`]); at least four, closing a finite solid
    /// ([`PreformSpec::from_planes`] checks).
    pub planes: Vec<(DVec3, f64)>,
    /// Where the set came from, for the Preform tab (e.g. "plan 'Aqua pebble', result 3,
    /// stone 1"); may be empty.
    pub source: String,
}

/// The rough's cross-sectional shape, viewed down the vertical (`y`) axis.
#[derive(Debug, Clone, PartialEq)]
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
    /// A convex plane set of its own (the Rough Planner's planned piece of rough, or any
    /// other half-space rough), in the design's own frame. The three size fields of the
    /// [`PreformSpec`] then describe the set's bounding box (width along `x`, length along
    /// `z`, height along `y`, as `measure_solid` reports them) and are informational:
    /// [`PreformSpec::planes`] returns the set itself, closed already, with no `+Y`/`-Y`
    /// planes added.
    Planes(Arc<PlaneSet>),
}

/// A design's uncut rough, as a small closed plane set.
///
/// `half_width`/`length_over_width`/`depth` are in the same "mast unit" scale as every
/// other plane offset in this codebase (a design's facets are masts of order 1;
/// `crate::design::Design` hands `PreformSpec::planes`'s output to
/// `indicatrix::geometry::stone_metrics::build_solid_mesh` in exactly that convention, no
/// rescaling).
#[derive(Debug, Clone, PartialEq)]
pub struct PreformSpec {
    /// Shape of the rough block.
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

    /// A preform from a convex plane set in model units (the design's frame). `None` unless the
    /// set closes into a finite solid of positive volume (`measure_solid`), in which case the
    /// size fields are its bounding box: `half_width = width_axis / 2`,
    /// `length_over_width = length_axis / width_axis`, `depth = total_height`.
    #[must_use]
    pub fn from_planes(planes: Vec<(DVec3, f64)>, source: String) -> Option<Self> {
        let metrics = measure_solid(&planes)?;
        if !(metrics.width_axis.is_finite() && metrics.width_axis > 0.0) {
            return None;
        }
        Some(Self {
            shape: PreformShape::Planes(Arc::new(PlaneSet { planes, source })),
            half_width: metrics.width_axis * 0.5,
            length_over_width: metrics.length_axis / metrics.width_axis,
            depth: metrics.total_height,
        })
    }

    /// Where a [`PreformShape::Planes`] preform came from (its [`PlaneSet::source`], possibly
    /// empty); `None` for a block or cylinder.
    #[must_use]
    pub fn source(&self) -> Option<&str> {
        match &self.shape {
            PreformShape::Planes(set) => Some(set.source.as_str()),
            PreformShape::Block | PreformShape::Cylinder { .. } => None,
        }
    }

    /// Whether this preform is a [`PreformShape::Planes`] set rather than a block or cylinder.
    #[must_use]
    pub const fn is_plane_set(&self) -> bool {
        matches!(self.shape, PreformShape::Planes(_))
    }

    /// The preform's own closed half-space plane set, in the `n . x <= m`
    /// convention [`indicatrix::geometry::stone_metrics::measure_solid`] and
    /// [`indicatrix::geometry::stone_metrics::build_solid_mesh`] take.
    ///
    /// Always a valid, finite, closed solid on its own (at least a
    /// triangular prism, 5 planes) -- this is the property
    /// [`crate::design::Design`] relies on to guarantee a fresh design (no
    /// facets yet) already renders as a real stone. A [`PreformShape::Planes`]
    /// preform returns its set as it is (closed by construction,
    /// [`Self::from_planes`] checks), with no `+Y`/`-Y` planes added.
    #[must_use]
    pub fn planes(&self) -> Vec<(DVec3, f64)> {
        let half_length = self.half_width * self.length_over_width;
        let half_depth = self.depth * 0.5;
        let mut planes = match &self.shape {
            PreformShape::Planes(set) => return set.planes.clone(),
            PreformShape::Block => vec![
                (DVec3::X, self.half_width),
                (DVec3::NEG_X, self.half_width),
                (DVec3::Z, half_length),
                (DVec3::NEG_Z, half_length),
            ],
            PreformShape::Cylinder { sides } => {
                let sides = (*sides).max(3);
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
    /// A [`PreformShape::Planes`] preform has no `+Y`/`-Y` pair to move; the general
    /// form applies instead: the whole solid moves up by `y_offset`, so every
    /// plane's offset becomes `m + y_offset * n.y`. For the two prism shapes that
    /// is exactly what the index-from-the-end code below does, and they keep it.
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
        if self.is_plane_set() {
            for (normal, offset) in &mut planes {
                *offset = y_offset.mul_add(normal.y, *offset);
            }
            return planes;
        }
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
    use indicatrix::geometry::stone_metrics::{SolidStatus, build_solid_mesh};

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

    /// The six planes of a block, given to `from_planes`, must measure the same
    /// volume and extents as the block and give the same size fields; the set
    /// is returned as it is (no `+Y`/`-Y` planes added).
    #[test]
    fn from_planes_of_a_block_matches_the_block() {
        let block = PreformSpec::block(1.0, 1.5, 0.8);
        let set = PreformSpec::from_planes(block.planes(), String::new())
            .expect("a block's planes close into a solid");
        assert!(set.is_plane_set());
        assert!(!block.is_plane_set());
        assert_eq!(set.planes(), block.planes());

        let expected = measure_solid(&block.planes()).unwrap();
        let measured = measure_solid(&set.planes()).unwrap();
        assert!((measured.volume - expected.volume).abs() < 1e-9);
        assert!((measured.width_axis - expected.width_axis).abs() < 1e-9);
        assert!((measured.length_axis - expected.length_axis).abs() < 1e-9);
        assert!((measured.total_height - expected.total_height).abs() < 1e-9);

        assert!((set.half_width - block.half_width).abs() < 1e-9);
        assert!((set.length_over_width - block.length_over_width).abs() < 1e-9);
        assert!((set.depth - block.depth).abs() < 1e-9);
    }

    /// `planes_offset` on a plane set moves the whole solid: the top and bottom
    /// by the offset, the height unchanged, and the result agrees with the block's
    /// own index-from-the-end shift.
    #[test]
    fn plane_set_planes_offset_moves_the_whole_solid() {
        let block = PreformSpec::block(1.0, 1.5, 0.8);
        let set = PreformSpec::from_planes(block.planes(), String::new()).unwrap();
        let shifted = set.planes_offset(0.3);
        let metrics = measure_solid(&shifted).expect("a shifted set still closes");
        assert!((metrics.total_height - 0.8).abs() < 1e-9);
        assert!((metrics.width_axis - 2.0).abs() < 1e-9);
        assert!((metrics.length_axis - 3.0).abs() < 1e-9);

        let top = block.depth * 0.5;
        assert!((shifted[shifted.len() - 2].1 - (top + 0.3)).abs() < 1e-9);
        assert!((shifted[shifted.len() - 1].1 - (top - 0.3)).abs() < 1e-9);

        let reference = block.planes_offset(0.3);
        assert_eq!(shifted.len(), reference.len());
        for (got, want) in shifted.iter().zip(&reference) {
            assert_eq!(got.0, want.0);
            assert!((got.1 - want.1).abs() < 1e-12, "{} vs {}", got.1, want.1);
        }
        assert_eq!(set.planes_offset(0.0), set.planes());
    }

    /// Three planes cannot close a finite solid, so there is no preform.
    #[test]
    fn a_non_closing_plane_set_is_not_a_preform() {
        let planes = vec![(DVec3::X, 1.0), (DVec3::NEG_X, 1.0), (DVec3::Z, 1.0)];
        assert!(PreformSpec::from_planes(planes, String::new()).is_none());
    }

    /// The set's source reaches the caller through `source()`; blocks and cylinders have none.
    #[test]
    fn a_plane_set_reports_its_source() {
        let block = PreformSpec::block(1.0, 1.0, 1.0);
        let set = PreformSpec::from_planes(block.planes(), "plan 'x'".to_string()).unwrap();
        assert_eq!(set.source(), Some("plan 'x'"));
        assert_eq!(block.source(), None);
        assert_eq!(PreformSpec::cylinder(8, 1.0, 1.0, 1.0).source(), None);
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
