//! The zones panel of the Fit step as data: the rows of the zone list, the parameter rows of the
//! selected zone, default shapes for "Add", and the refinement settings.
//!
//! The edits themselves are
//! `colour_fit::zones::apply_with_locks`; this module only words and parses.

use glam::DVec3;
use indicatrix::optics::{
    absorption::AbsorptionTensor,
    zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
};
use indicatrix_cut_core::rough_plan::colour_fit::zones::{
    DEFAULT_REFINE_ITERATIONS, MAX_REFINE_ITERATIONS, RefineOptions, ZoneEdit, ZoneLocks,
    ZoneParameter, parameter_value,
};

/// The primitives the panel can add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeKind {
    /// One side of a plane.
    HalfSpace,
    /// Between two parallel planes.
    Slab,
    /// A cylinder about an axis.
    Cylinder,
    /// A prism about an axis.
    Prism,
    /// A wedge about an axis.
    Sector,
}

impl ShapeKind {
    /// All kinds, in the order of the panel's pills.
    pub const ALL: [Self; 5] = [
        Self::HalfSpace,
        Self::Slab,
        Self::Cylinder,
        Self::Prism,
        Self::Sector,
    ];

    /// The name on the pill.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::HalfSpace => "Half space",
            Self::Slab => "Slab",
            Self::Cylinder => "Cylinder",
            Self::Prism => "Prism",
            Self::Sector => "Sector",
        }
    }

    /// The kind at a list index.
    #[must_use]
    pub fn from_index(index: i32) -> Option<Self> {
        usize::try_from(index)
            .ok()
            .and_then(|i| Self::ALL.get(i).copied())
    }
}

/// A starting shape for a rough centred at `centre` with `extent_mm` across.
///
/// A plane through the centre, a slab of a quarter of the extent, a cylinder or prism of a quarter of the extent along
/// the vertical axis, a quarter-turn sector.
#[must_use]
pub fn default_shape(kind: ShapeKind, centre: DVec3, extent_mm: f64) -> ZoneShape {
    let extent = if extent_mm.is_finite() && extent_mm > 0.0 {
        extent_mm
    } else {
        10.0
    };
    match kind {
        ShapeKind::HalfSpace => ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset: centre.x,
        },
        ShapeKind::Slab => ZoneShape::Slab {
            normal: DVec3::X,
            offset_min: extent.mul_add(-0.125, centre.x),
            offset_max: extent.mul_add(0.125, centre.x),
        },
        ShapeKind::Cylinder => ZoneShape::CoaxialCylinder {
            axis_point: centre,
            axis_dir: DVec3::Z,
            r_in: 0.0,
            r_out: extent * 0.25,
        },
        ShapeKind::Prism => ZoneShape::CoaxialPrism {
            axis_point: centre,
            axis_dir: DVec3::Z,
            n_sides: 3,
            r_in: 0.0,
            r_out: extent * 0.25,
            phase: 0.0,
        },
        ShapeKind::Sector => ZoneShape::Sector {
            axis_point: centre,
            axis_dir: DVec3::Z,
            angle_from: 0.0,
            angle_to: std::f64::consts::FRAC_PI_2,
        },
    }
}

/// A zone with a clear placeholder absorption: the fit replaces it with the fitted one, the
/// geometry is what the user edits.
#[must_use]
pub fn placeholder_zone(shape: ZoneShape) -> Zone {
    Zone {
        shape,
        absorption: ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new())),
    }
}

/// The kind name of a shape.
#[must_use]
pub const fn shape_name(shape: &ZoneShape) -> &'static str {
    match shape {
        ZoneShape::HalfSpace { .. } => "Half space",
        ZoneShape::Slab { .. } => "Slab",
        ZoneShape::CoaxialCylinder { .. } => "Cylinder",
        ZoneShape::CoaxialPrism { .. } => "Prism",
        ZoneShape::Sector { .. } => "Sector",
        ZoneShape::MeshShell { .. } => "Mesh shell",
    }
}

/// A row of the zone list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneRow {
    /// `"Base zone"` or `"Zone 2: Cylinder"`.
    pub title: String,
    /// The key numbers.
    pub summary: String,
}

/// The editable scalar parameters of a shape, in display order.
#[must_use]
pub fn scalar_parameters(shape: &ZoneShape) -> Vec<ZoneParameter> {
    match shape {
        ZoneShape::HalfSpace { .. } => vec![ZoneParameter::Offset],
        ZoneShape::Slab { .. } => vec![ZoneParameter::OffsetMin, ZoneParameter::OffsetMax],
        ZoneShape::CoaxialCylinder { .. } => vec![ZoneParameter::RIn, ZoneParameter::ROut],
        ZoneShape::CoaxialPrism { .. } => vec![
            ZoneParameter::NSides,
            ZoneParameter::RIn,
            ZoneParameter::ROut,
            ZoneParameter::Phase,
        ],
        ZoneShape::Sector { .. } => vec![ZoneParameter::AngleFrom, ZoneParameter::AngleTo],
        ZoneShape::MeshShell { .. } => Vec::new(),
    }
}

fn number(value: f64) -> String {
    let text = format!("{value:.3}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" || trimmed == "-0" {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The rows of the zone list: the base zone and every zone, with its numbers.
#[must_use]
pub fn zone_rows(zoned: &ZonedAbsorption) -> Vec<ZoneRow> {
    let mut rows = vec![ZoneRow {
        title: "Base zone".to_owned(),
        summary: "Everywhere no zone covers".to_owned(),
    }];
    for (i, zone) in zoned.zones.iter().enumerate() {
        let parts: Vec<String> = scalar_parameters(&zone.shape)
            .into_iter()
            .filter_map(|p| {
                parameter_value(&zone.shape, p).map(|v| format!("{} {}", p.name(), number(v)))
            })
            .collect();
        rows.push(ZoneRow {
            title: format!("Zone {}: {}", i + 1, shape_name(&zone.shape)),
            summary: parts.join(", "),
        });
    }
    rows
}

/// A parameter row of the selected zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamRow {
    /// The name.
    pub name: String,
    /// The value as text.
    pub value: String,
    /// Whether the refinement keeps it fixed.
    pub locked: bool,
}

/// The parameter rows of zone `zone` (1-based length-array index; 0 is the base zone, which has
/// no geometry).
#[must_use]
pub fn param_rows(zoned: &ZonedAbsorption, locks: &ZoneLocks, zone: usize) -> Vec<ParamRow> {
    let Some(entry) = zone.checked_sub(1).and_then(|i| zoned.zones.get(i)) else {
        return Vec::new();
    };
    scalar_parameters(&entry.shape)
        .into_iter()
        .filter_map(|p| {
            parameter_value(&entry.shape, p).map(|v| ParamRow {
                name: p.name().to_owned(),
                value: number(v),
                locked: locks.is_locked(zone, p),
            })
        })
        .collect()
}

/// The edit that sets parameter number `row` of zone `zone` to the typed text.
///
/// # Errors
///
/// A sentence when the zone or row does not exist or the text is not a number.
pub fn edit_from_text(
    zoned: &ZonedAbsorption,
    zone: usize,
    row: usize,
    text: &str,
) -> Result<ZoneEdit, String> {
    let entry = zone
        .checked_sub(1)
        .and_then(|i| zoned.zones.get(i))
        .ok_or_else(|| "Pick a zone first.".to_owned())?;
    let parameter = *scalar_parameters(&entry.shape)
        .get(row)
        .ok_or_else(|| "That parameter does not exist.".to_owned())?;
    let value: f64 = text
        .trim()
        .replace(',', ".")
        .parse()
        .map_err(|_| format!("\"{}\" is not a number.", text.trim()))?;
    if !value.is_finite() {
        return Err("The value must be a finite number.".to_owned());
    }
    Ok(ZoneEdit::SetParameter {
        zone,
        parameter,
        value,
    })
}

/// The parameter of row `row` of zone `zone`, for a lock toggle.
#[must_use]
pub fn parameter_of_row(zoned: &ZonedAbsorption, zone: usize, row: usize) -> Option<ZoneParameter> {
    let entry = zone.checked_sub(1).and_then(|i| zoned.zones.get(i))?;
    scalar_parameters(&entry.shape).get(row).copied()
}

/// The range of the "Refinement iterations" spin box.
pub const ITERATIONS_RANGE: std::ops::RangeInclusive<i32> = 1..=(MAX_REFINE_ITERATIONS as i32);

/// The tooltip of the spin box.
pub const ITERATIONS_HINT: &str = "How many rounds the zone boundaries are moved at most. Each round costs (free parameters + 1) full trace-and-fit passes, and refining stops early when a step no longer improves the fit.";

/// The iteration count typed into the spin box, read leniently: empty or bad text gives the
/// default, anything outside `1..=25` is clamped.
#[must_use]
pub fn iterations_from_text(text: &str) -> usize {
    text.trim()
        .parse::<i64>()
        .ok()
        .map_or(DEFAULT_REFINE_ITERATIONS, |n| {
            n.clamp(1, MAX_REFINE_ITERATIONS as i64) as usize
        })
}

/// The refinement settings for `iterations` rounds.
#[must_use]
pub fn refine_options(iterations: usize) -> RefineOptions {
    RefineOptions {
        max_iterations: iterations.clamp(1, MAX_REFINE_ITERATIONS),
        ..RefineOptions::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::colour_fit::zones::apply;

    fn with_zone(kind: ShapeKind) -> ZonedAbsorption {
        let base = ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new()));
        let zoned = ZonedAbsorption::new(base);
        apply(
            &zoned,
            &ZoneEdit::Add {
                zone: placeholder_zone(default_shape(kind, DVec3::new(1.0, 2.0, 3.0), 12.0)),
                position: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn every_default_shape_is_a_valid_zone() {
        for kind in ShapeKind::ALL {
            let zoned = with_zone(kind);
            assert_eq!(zoned.zones.len(), 1, "{kind:?}");
            assert!(zoned.validate().is_ok(), "{kind:?}");
            assert_eq!(shape_name(&zoned.zones[0].shape), kind.name());
        }
        // A broken extent falls back.
        let shape = default_shape(ShapeKind::Cylinder, DVec3::ZERO, f64::NAN);
        assert!(matches!(shape, ZoneShape::CoaxialCylinder { r_out, .. } if r_out > 0.0));
    }

    #[test]
    fn the_list_names_the_zones_and_their_numbers() {
        let rows = zone_rows(&with_zone(ShapeKind::Slab));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "Base zone");
        assert_eq!(rows[1].title, "Zone 1: Slab");
        assert_eq!(rows[1].summary, "lower offset -0.5, upper offset 2.5");
    }

    #[test]
    fn parameter_rows_follow_the_shape_and_the_locks() {
        let zoned = with_zone(ShapeKind::Prism);
        let mut locks = ZoneLocks::new();
        let rows = param_rows(&zoned, &locks, 1);
        let names: Vec<_> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            ["side count", "inner radius", "outer radius", "phase"]
        );
        assert!(rows.iter().all(|r| !r.locked));
        let (_, with_lock) = indicatrix_cut_core::rough_plan::colour_fit::zones::apply_with_locks(
            &zoned,
            &locks,
            &ZoneEdit::Lock {
                zone: 1,
                parameter: ZoneParameter::ROut,
            },
        )
        .unwrap();
        locks = with_lock;
        assert!(param_rows(&zoned, &locks, 1)[2].locked);
        assert_eq!(param_rows(&zoned, &locks, 0), [] as [ParamRow; 0]);
        assert_eq!(param_rows(&zoned, &locks, 5), [] as [ParamRow; 0]);
        assert_eq!(parameter_of_row(&zoned, 1, 2), Some(ZoneParameter::ROut));
        assert_eq!(parameter_of_row(&zoned, 1, 9), None);
    }

    #[test]
    fn typed_values_become_edits() {
        let zoned = with_zone(ShapeKind::HalfSpace);
        let edit = edit_from_text(&zoned, 1, 0, " 2,5 ").unwrap();
        assert_eq!(
            edit,
            ZoneEdit::SetParameter {
                zone: 1,
                parameter: ZoneParameter::Offset,
                value: 2.5
            }
        );
        let moved = apply(&zoned, &edit).unwrap();
        assert_eq!(zone_rows(&moved)[1].summary, "offset 2.5");
        assert!(
            edit_from_text(&zoned, 1, 0, "abc")
                .unwrap_err()
                .contains("not a number")
        );
        assert!(edit_from_text(&zoned, 1, 3, "1").is_err());
        assert!(edit_from_text(&zoned, 0, 0, "1").is_err());
        assert!(edit_from_text(&zoned, 1, 0, "inf").is_err());
    }

    #[test]
    fn the_refinement_iterations_are_read_leniently_and_reach_the_options() {
        assert_eq!(iterations_from_text("7"), 7);
        assert_eq!(iterations_from_text(""), DEFAULT_REFINE_ITERATIONS);
        assert_eq!(iterations_from_text("x"), DEFAULT_REFINE_ITERATIONS);
        assert_eq!(iterations_from_text("0"), 1);
        assert_eq!(iterations_from_text("500"), MAX_REFINE_ITERATIONS);
        assert_eq!(iterations_from_text("-4"), 1);
        assert_eq!(refine_options(9).max_iterations, 9);
        assert_eq!(refine_options(0).max_iterations, 1);
        assert_eq!(refine_options(99).max_iterations, MAX_REFINE_ITERATIONS);
        assert_eq!(DEFAULT_REFINE_ITERATIONS, 3);
        assert_eq!(*ITERATIONS_RANGE.end(), 25);
        assert!(ITERATIONS_HINT.contains("free parameters + 1"));
    }

    #[test]
    fn numbers_print_without_trailing_zeros() {
        assert_eq!(number(1.0), "1");
        assert_eq!(number(0.25), "0.25");
        assert_eq!(number(-0.0001), "0");
        assert_eq!(number(-2.5), "-2.5");
    }
}
