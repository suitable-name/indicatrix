//! The editing backend of the zone geometry: pure operations on a
//! [`ZonedAbsorption`], each validated through the kernels' `validate`, plus the parameter locks
//! the joint refinement respects.
//!
//! Zone indices follow the kernels' length-array order: 0 is the base zone (no geometry), 1.. the
//! entries of [`ZonedAbsorption::zones`].
//!
//! [`apply`] never changes its input; an edit that would leave the zoning invalid, or that touches
//! a locked parameter, returns an error and nothing else happens (undo is "keep the old value").
//! The locks are not part of [`ZonedAbsorption`] (a saved zoning does not carry them), so they
//! travel beside it in [`ZoneLocks`]; [`apply_with_locks`] keeps both consistent when zones are
//! added, removed or reordered. Plain [`apply`] has no locks: lock and unlock edits return the
//! zoning unchanged.

use std::{collections::BTreeSet, fmt};

use glam::DVec3;
use indicatrix::optics::zoning::{
    MAX_ZONES, Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption, ZoningError,
};

/// One editable (and lockable) parameter of a zone shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ZoneParameter {
    /// The plane offset of a half space.
    Offset,
    /// The lower plane offset of a slab.
    OffsetMin,
    /// The upper plane offset of a slab.
    OffsetMax,
    /// The inner radius (apothem) of a cylinder or prism.
    RIn,
    /// The outer radius (apothem) of a cylinder or prism.
    ROut,
    /// The phase of a prism.
    Phase,
    /// The side count of a prism.
    NSides,
    /// The start angle of a sector.
    AngleFrom,
    /// The end angle of a sector.
    AngleTo,
    /// The normal of a half space or slab, the axis direction of the others.
    Direction,
    /// The axis point of a cylinder, prism or sector.
    AxisPoint,
}

impl ZoneParameter {
    /// A short name for messages.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Offset => "offset",
            Self::OffsetMin => "lower offset",
            Self::OffsetMax => "upper offset",
            Self::RIn => "inner radius",
            Self::ROut => "outer radius",
            Self::Phase => "phase",
            Self::NSides => "side count",
            Self::AngleFrom => "start angle",
            Self::AngleTo => "end angle",
            Self::Direction => "direction",
            Self::AxisPoint => "axis point",
        }
    }
}

/// An edit of the zoning.
#[derive(Debug, Clone, PartialEq)]
pub enum ZoneEdit {
    /// Adds a zone at `position` (a zone index, 1 to one past the last; `None` appends). Later
    /// zones override earlier ones, so the position is the stacking order.
    Add {
        /// The new zone.
        zone: Zone,
        /// Where to insert it.
        position: Option<usize>,
    },
    /// Removes a zone (not the base).
    Remove {
        /// The zone index (1..).
        zone: usize,
    },
    /// Moves a zone to another place in the stacking order.
    Reorder {
        /// The zone index it has now (1..).
        from: usize,
        /// The zone index it should have (1..).
        to: usize,
    },
    /// Replaces the geometry of a zone (for example with an accepted suggestion or a fit).
    SetShape {
        /// The zone index (1..).
        zone: usize,
        /// The new shape.
        shape: ZoneShape,
    },
    /// Sets one scalar parameter.
    SetParameter {
        /// The zone index (1..).
        zone: usize,
        /// Which parameter.
        parameter: ZoneParameter,
        /// The new value (mm, radians, or the side count).
        value: f64,
    },
    /// Sets the normal / axis direction (normalised on the way in).
    SetDirection {
        /// The zone index (1..).
        zone: usize,
        /// The direction; it must be finite and not zero.
        direction: DVec3,
    },
    /// Sets the axis point.
    SetAxisPoint {
        /// The zone index (1..).
        zone: usize,
        /// The point, mm.
        point: DVec3,
    },
    /// Sets the absorption of a zone; zone 0 is the base.
    SetAbsorption {
        /// The zone index (0 is the base).
        zone: usize,
        /// The new absorption.
        absorption: ZoneAbsorption,
    },
    /// Locks a parameter against edits and against the joint refinement.
    Lock {
        /// The zone index (1..).
        zone: usize,
        /// Which parameter.
        parameter: ZoneParameter,
    },
    /// Releases a lock.
    Unlock {
        /// The zone index (1..).
        zone: usize,
        /// Which parameter.
        parameter: ZoneParameter,
    },
    /// Sets the boundary softness, mm (0 is sharp).
    SetSoftness {
        /// The width of the smoothstep blend.
        millimetres: f32,
    },
}

/// Why an edit was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZoneEditError {
    /// The edited zoning does not validate.
    Invalid(ZoningError),
    /// There is no such zone.
    NoSuchZone {
        /// The zone index asked for.
        zone: usize,
    },
    /// The base zone has no geometry.
    BaseHasNoGeometry,
    /// The zone's shape has no such parameter.
    NoSuchParameter {
        /// The zone index.
        zone: usize,
        /// The parameter.
        parameter: ZoneParameter,
    },
    /// The parameter is locked.
    Locked {
        /// The zone index.
        zone: usize,
        /// The parameter.
        parameter: ZoneParameter,
    },
    /// A value is not finite (or a direction is zero).
    NotFinite,
    /// The insertion or move position is outside 1 to the number of zones (plus one for an
    /// insertion).
    BadPosition {
        /// The position given.
        position: usize,
    },
}

impl fmt::Display for ZoneEditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(e) => write!(f, "{e}"),
            Self::NoSuchZone { zone } => write!(f, "there is no zone {zone}"),
            Self::BaseHasNoGeometry => write!(f, "the base zone has no geometry to edit"),
            Self::NoSuchParameter { zone, parameter } => {
                write!(f, "zone {zone} has no {}", parameter.name())
            }
            Self::Locked { zone, parameter } => {
                write!(f, "the {} of zone {zone} is locked", parameter.name())
            }
            Self::NotFinite => write!(f, "the value is not a usable number"),
            Self::BadPosition { position } => {
                write!(f, "position {position} is outside the stacking order")
            }
        }
    }
}

impl std::error::Error for ZoneEditError {}

impl From<ZoningError> for ZoneEditError {
    fn from(e: ZoningError) -> Self {
        Self::Invalid(e)
    }
}

/// The locked parameters: pairs of a zone index and a parameter.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ZoneLocks {
    locked: BTreeSet<(usize, ZoneParameter)>,
}

impl ZoneLocks {
    /// No locks.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            locked: BTreeSet::new(),
        }
    }

    /// Whether the parameter of the zone is locked.
    #[must_use]
    pub fn is_locked(&self, zone: usize, parameter: ZoneParameter) -> bool {
        self.locked.contains(&(zone, parameter))
    }

    /// The locks in order.
    pub fn iter(&self) -> impl Iterator<Item = (usize, ZoneParameter)> + '_ {
        self.locked.iter().copied()
    }

    /// The number of locks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.locked.len()
    }

    /// Whether nothing is locked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.locked.is_empty()
    }

    fn remapped(&self, map: impl Fn(usize) -> Option<usize>) -> Self {
        Self {
            locked: self
                .locked
                .iter()
                .filter_map(|&(zone, parameter)| map(zone).map(|z| (z, parameter)))
                .collect(),
        }
    }
}

/// The scalar value of a parameter, if the shape has it.
#[must_use]
pub fn parameter_value(shape: &ZoneShape, parameter: ZoneParameter) -> Option<f64> {
    match (shape, parameter) {
        (ZoneShape::HalfSpace { offset, .. }, ZoneParameter::Offset) => Some(*offset),
        (ZoneShape::Slab { offset_min, .. }, ZoneParameter::OffsetMin) => Some(*offset_min),
        (ZoneShape::Slab { offset_max, .. }, ZoneParameter::OffsetMax) => Some(*offset_max),
        (
            ZoneShape::CoaxialCylinder { r_in, .. } | ZoneShape::CoaxialPrism { r_in, .. },
            ZoneParameter::RIn,
        ) => Some(*r_in),
        (
            ZoneShape::CoaxialCylinder { r_out, .. } | ZoneShape::CoaxialPrism { r_out, .. },
            ZoneParameter::ROut,
        ) => Some(*r_out),
        (ZoneShape::CoaxialPrism { phase, .. }, ZoneParameter::Phase) => Some(*phase),
        (ZoneShape::CoaxialPrism { n_sides, .. }, ZoneParameter::NSides) => {
            Some(f64::from(*n_sides))
        }
        (ZoneShape::Sector { angle_from, .. }, ZoneParameter::AngleFrom) => Some(*angle_from),
        (ZoneShape::Sector { angle_to, .. }, ZoneParameter::AngleTo) => Some(*angle_to),
        _ => None,
    }
}

/// Sets a scalar parameter; `false` when the shape has no such parameter or the value cannot be
/// stored (a side count that is not a non-negative integer).
pub(super) fn set_parameter_value(
    shape: &mut ZoneShape,
    parameter: ZoneParameter,
    value: f64,
) -> bool {
    match (shape, parameter) {
        (ZoneShape::HalfSpace { offset, .. }, ZoneParameter::Offset) => *offset = value,
        (ZoneShape::Slab { offset_min, .. }, ZoneParameter::OffsetMin) => *offset_min = value,
        (ZoneShape::Slab { offset_max, .. }, ZoneParameter::OffsetMax) => *offset_max = value,
        (
            ZoneShape::CoaxialCylinder { r_in, .. } | ZoneShape::CoaxialPrism { r_in, .. },
            ZoneParameter::RIn,
        ) => *r_in = value,
        (
            ZoneShape::CoaxialCylinder { r_out, .. } | ZoneShape::CoaxialPrism { r_out, .. },
            ZoneParameter::ROut,
        ) => *r_out = value,
        (ZoneShape::CoaxialPrism { phase, .. }, ZoneParameter::Phase) => *phase = value,
        (ZoneShape::CoaxialPrism { n_sides, .. }, ZoneParameter::NSides) => {
            let rounded = value.round();
            if !(0.0..=1000.0).contains(&rounded) {
                return false;
            }
            *n_sides = rounded as u32;
        }
        (ZoneShape::Sector { angle_from, .. }, ZoneParameter::AngleFrom) => *angle_from = value,
        (ZoneShape::Sector { angle_to, .. }, ZoneParameter::AngleTo) => *angle_to = value,
        _ => return false,
    }
    true
}

const fn direction_of(shape: &ZoneShape) -> Option<DVec3> {
    match shape {
        ZoneShape::HalfSpace { normal, .. } | ZoneShape::Slab { normal, .. } => Some(*normal),
        ZoneShape::CoaxialCylinder { axis_dir, .. }
        | ZoneShape::CoaxialPrism { axis_dir, .. }
        | ZoneShape::Sector { axis_dir, .. } => Some(*axis_dir),
        ZoneShape::MeshShell { .. } => None,
    }
}

const fn axis_point_of(shape: &ZoneShape) -> Option<DVec3> {
    match shape {
        ZoneShape::CoaxialCylinder { axis_point, .. }
        | ZoneShape::CoaxialPrism { axis_point, .. }
        | ZoneShape::Sector { axis_point, .. } => Some(*axis_point),
        _ => None,
    }
}

const fn set_direction(shape: &mut ZoneShape, direction: DVec3) -> bool {
    match shape {
        ZoneShape::HalfSpace { normal, .. } | ZoneShape::Slab { normal, .. } => {
            *normal = direction;
            true
        }
        ZoneShape::CoaxialCylinder { axis_dir, .. }
        | ZoneShape::CoaxialPrism { axis_dir, .. }
        | ZoneShape::Sector { axis_dir, .. } => {
            *axis_dir = direction;
            true
        }
        ZoneShape::MeshShell { .. } => false,
    }
}

const fn set_axis_point(shape: &mut ZoneShape, point: DVec3) -> bool {
    match shape {
        ZoneShape::CoaxialCylinder { axis_point, .. }
        | ZoneShape::CoaxialPrism { axis_point, .. }
        | ZoneShape::Sector { axis_point, .. } => {
            *axis_point = point;
            true
        }
        _ => false,
    }
}

/// The parameters of a shape that can be locked and refined by hand.
const SCALARS: [ZoneParameter; 9] = [
    ZoneParameter::Offset,
    ZoneParameter::OffsetMin,
    ZoneParameter::OffsetMax,
    ZoneParameter::RIn,
    ZoneParameter::ROut,
    ZoneParameter::Phase,
    ZoneParameter::NSides,
    ZoneParameter::AngleFrom,
    ZoneParameter::AngleTo,
];

/// The parameters (of `SCALARS`, `Direction`, `AxisPoint`) in which two shapes of the same kind
/// differ, and the ones one has and the other lacks.
fn differing_parameters(old: &ZoneShape, new: &ZoneShape) -> Vec<ZoneParameter> {
    let mut out = Vec::new();
    for p in SCALARS {
        if parameter_value(old, p) != parameter_value(new, p) {
            out.push(p);
        }
    }
    if direction_of(old) != direction_of(new) {
        out.push(ZoneParameter::Direction);
    }
    if axis_point_of(old) != axis_point_of(new) {
        out.push(ZoneParameter::AxisPoint);
    }
    out
}

fn zone_mut(zoned: &mut ZonedAbsorption, zone: usize) -> Result<&mut Zone, ZoneEditError> {
    if zone == 0 {
        return Err(ZoneEditError::BaseHasNoGeometry);
    }
    zoned
        .zones
        .get_mut(zone - 1)
        .ok_or(ZoneEditError::NoSuchZone { zone })
}

fn check_unlocked(
    locks: &ZoneLocks,
    zone: usize,
    parameter: ZoneParameter,
) -> Result<(), ZoneEditError> {
    if locks.is_locked(zone, parameter) {
        Err(ZoneEditError::Locked { zone, parameter })
    } else {
        Ok(())
    }
}

/// Applies `edit` to `zoned` with no locks. The result is validated; the input is untouched.
///
/// [`ZoneEdit::Lock`] and [`ZoneEdit::Unlock`] return the zoning unchanged (use
/// [`apply_with_locks`] to keep the locks).
///
/// # Errors
///
/// [`ZoneEditError`]: a bad index or position, a parameter the shape lacks, a value that is not
/// finite, or a result that fails `ZonedAbsorption::validate`.
pub fn apply(zoned: &ZonedAbsorption, edit: &ZoneEdit) -> Result<ZonedAbsorption, ZoneEditError> {
    apply_with_locks(zoned, &ZoneLocks::new(), edit).map(|(z, _)| z)
}

/// Applies `edit` to `zoned` and `locks` together: editing a locked parameter is refused, and
/// adding, removing and reordering zones renumber the locks.
///
/// # Errors
///
/// As [`apply`], plus [`ZoneEditError::Locked`].
pub fn apply_with_locks(
    zoned: &ZonedAbsorption,
    locks: &ZoneLocks,
    edit: &ZoneEdit,
) -> Result<(ZonedAbsorption, ZoneLocks), ZoneEditError> {
    let mut out = zoned.clone();
    let mut new_locks = locks.clone();
    match edit {
        ZoneEdit::Add { zone, position } => {
            if out.zones.len() >= MAX_ZONES {
                return Err(ZoningError::TooManyZones {
                    count: out.zones.len() + 1,
                }
                .into());
            }
            let at = position.unwrap_or(out.zones.len() + 1);
            if at == 0 || at > out.zones.len() + 1 {
                return Err(ZoneEditError::BadPosition { position: at });
            }
            out.zones.insert(at - 1, zone.clone());
            new_locks = locks.remapped(|z| Some(if z >= at { z + 1 } else { z }));
        }
        ZoneEdit::Remove { zone } => {
            let _ = zone_mut(&mut out, *zone)?;
            out.zones.remove(zone - 1);
            let removed = *zone;
            new_locks = locks.remapped(|z| match z.cmp(&removed) {
                std::cmp::Ordering::Less => Some(z),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some(z - 1),
            });
        }
        ZoneEdit::Reorder { from, to } => {
            let count = out.zones.len();
            if *from == 0 || *from > count {
                return Err(ZoneEditError::NoSuchZone { zone: *from });
            }
            if *to == 0 || *to > count {
                return Err(ZoneEditError::BadPosition { position: *to });
            }
            let moved = out.zones.remove(from - 1);
            out.zones.insert(to - 1, moved);
            let (from, to) = (*from, *to);
            new_locks = locks.remapped(|z| {
                Some(if z == from {
                    to
                } else if from < to && z > from && z <= to {
                    z - 1
                } else if to < from && z >= to && z < from {
                    z + 1
                } else {
                    z
                })
            });
        }
        ZoneEdit::SetShape { zone, shape } => {
            let target = zone_mut(&mut out, *zone)?;
            if std::mem::discriminant(&target.shape) == std::mem::discriminant(shape) {
                for parameter in differing_parameters(&target.shape, shape) {
                    check_unlocked(locks, *zone, parameter)?;
                }
            } else {
                for (z, parameter) in locks.iter() {
                    if z == *zone {
                        return Err(ZoneEditError::Locked { zone: z, parameter });
                    }
                }
            }
            target.shape = shape.clone();
        }
        ZoneEdit::SetParameter {
            zone,
            parameter,
            value,
        } => {
            if !value.is_finite() {
                return Err(ZoneEditError::NotFinite);
            }
            check_unlocked(locks, *zone, *parameter)?;
            let target = zone_mut(&mut out, *zone)?;
            if !set_parameter_value(&mut target.shape, *parameter, *value) {
                return Err(ZoneEditError::NoSuchParameter {
                    zone: *zone,
                    parameter: *parameter,
                });
            }
        }
        ZoneEdit::SetDirection { zone, direction } => {
            let Some(unit) = direction.try_normalize().filter(|d| d.is_finite()) else {
                return Err(ZoneEditError::NotFinite);
            };
            check_unlocked(locks, *zone, ZoneParameter::Direction)?;
            let target = zone_mut(&mut out, *zone)?;
            if !set_direction(&mut target.shape, unit) {
                return Err(ZoneEditError::NoSuchParameter {
                    zone: *zone,
                    parameter: ZoneParameter::Direction,
                });
            }
        }
        ZoneEdit::SetAxisPoint { zone, point } => {
            if !point.is_finite() {
                return Err(ZoneEditError::NotFinite);
            }
            check_unlocked(locks, *zone, ZoneParameter::AxisPoint)?;
            let target = zone_mut(&mut out, *zone)?;
            if !set_axis_point(&mut target.shape, *point) {
                return Err(ZoneEditError::NoSuchParameter {
                    zone: *zone,
                    parameter: ZoneParameter::AxisPoint,
                });
            }
        }
        ZoneEdit::SetAbsorption { zone, absorption } => {
            if *zone == 0 {
                out.base = absorption.clone();
            } else {
                zone_mut(&mut out, *zone)?.absorption = absorption.clone();
            }
        }
        ZoneEdit::Lock { zone, parameter } => {
            let target = zone_mut(&mut out, *zone)?;
            let present = match parameter {
                ZoneParameter::Direction => direction_of(&target.shape).is_some(),
                ZoneParameter::AxisPoint => axis_point_of(&target.shape).is_some(),
                p => parameter_value(&target.shape, *p).is_some(),
            };
            if !present {
                return Err(ZoneEditError::NoSuchParameter {
                    zone: *zone,
                    parameter: *parameter,
                });
            }
            new_locks.locked.insert((*zone, *parameter));
        }
        ZoneEdit::Unlock { zone, parameter } => {
            new_locks.locked.remove(&(*zone, *parameter));
        }
        ZoneEdit::SetSoftness { millimetres } => {
            if !millimetres.is_finite() {
                return Err(ZoneEditError::NotFinite);
            }
            out.boundary_softness_mm = *millimetres;
        }
    }
    out.validate()?;
    Ok((out, new_locks))
}
