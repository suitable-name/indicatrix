//! The orbit model itself: [`OrbitUnit`], [`orbit_units`] (grouping a tier's
//! raw `indices` into units), and [`expected_orbit`] (the reverse -- every
//! position one requested occurrence's orbit *should* have). See the parent
//! module's doc comment for the rotation/mirror math this implements and the
//! corpus measurement behind it.

use crate::design::ScheduleMeta;

/// Float-equality tolerance for index-wheel azimuths.
///
/// `indicatrix_formats::asc`'s own module docs measure roughly 0.2% of real index
/// tokens as fractional (non-integer index-wheel positions), so exact
/// equality is never the right test here; this is small enough to never
/// conflate two genuinely distinct index-wheel teeth (whole teeth are
/// always >= 1 apart) while absorbing that fractional noise and ordinary
/// `f64` rounding.
pub const INDEX_TOLERANCE: f64 = 1e-3;

/// Distance between `a` and `b` on a ring of circumference `ring` -- the
/// shorter of the two arcs between them, so a value pinned at
/// `ring - epsilon` and one at `0` compare as adjacent instead of as
/// almost `ring` apart.
///
/// Both [`orbit_units`] (residues, ring size `step`) and [`expected_orbit`]
/// (gear positions, ring size `gear_teeth_abs`) produce values through
/// `rem_euclid`, which wraps a geometrically-exact `0` around to
/// `ring - epsilon` under ordinary `f64` rounding. A linear
/// `(a - b).abs()` comparison misses that wrap, so every index/residue/
/// gear-position comparison in this module and [`super::edit`] must go
/// through this instead. `ring` must be strictly positive; callers only
/// ever reach it after checking `symmetry_order`/`gear_teeth_abs` are
/// both non-zero (the only case a ring exists at all).
#[must_use]
pub fn ring_distance(a: f64, b: f64, ring: f64) -> f64 {
    let d = (a - b).abs().rem_euclid(ring);
    d.min(ring - d)
}

/// One physical facet's occurrence set under the schedule's stated
/// symmetry -- see the module docs for the model and the corpus-measured
/// shape distribution.
#[derive(Debug, Clone, PartialEq)]
pub struct OrbitUnit {
    /// Members of this unit actually present in the tier's `indices`,
    /// ascending.
    pub members: Vec<f64>,
    /// How many members a *complete* orbit at this unit's residue would
    /// have (`symmetry_order`, or `2 * symmetry_order` for an off-axis
    /// facet under a mirrored schedule -- see the module docs).
    pub expected_len: usize,
}

impl OrbitUnit {
    /// `true` iff every member this unit's orbit needs is present.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.members.len() == self.expected_len
    }
}

fn is_axis_residue(residue: f64, step: f64) -> bool {
    ring_distance(residue, 0.0, step) < INDEX_TOLERANCE * 4.0
        || ring_distance(residue, step / 2.0, step) < INDEX_TOLERANCE * 4.0
}

/// Groups `indices` into their orbit units under `meta`'s
/// `symmetry_order`/`mirror`/`gear_teeth_abs` -- see the module docs.
///
/// Degenerates to one trivial (`expected_len: 1`) unit per index when
/// `symmetry_order` or `gear_teeth_abs` is `0` (a schedule that states no
/// usable symmetry at all): there is no lattice to group by, so every
/// occurrence is its own orbit rather than this function guessing one.
/// Deterministic and total-order (`Vec`s throughout, no `HashMap`/
/// `HashSet`): two calls against equal input always produce units in the
/// same order.
#[must_use]
pub fn orbit_units(indices: &[f64], meta: &ScheduleMeta) -> Vec<OrbitUnit> {
    let symmetry_order = meta.symmetry_order;
    let gear_teeth_abs = meta.gear_teeth_abs();
    if symmetry_order == 0 || gear_teeth_abs == 0 {
        return indices
            .iter()
            .map(|&v| OrbitUnit {
                members: vec![v],
                expected_len: 1,
            })
            .collect();
    }
    let step = f64::from(gear_teeth_abs) / f64::from(symmetry_order);

    // Pass 1: cluster raw indices by residue mod step (every member of one
    // rotational orbit shares a residue -- see the module docs).
    let mut clusters: Vec<(f64, Vec<f64>)> = Vec::new();
    for &idx in indices {
        let mut residue = idx.rem_euclid(step);
        if ring_distance(residue, 0.0, step) < INDEX_TOLERANCE {
            residue = 0.0;
        }
        match clusters
            .iter_mut()
            .find(|(r, _)| ring_distance(*r, residue, step) < INDEX_TOLERANCE * 4.0)
        {
            Some((_, members)) => members.push(idx),
            None => clusters.push((residue, vec![idx])),
        }
    }

    // Pass 2: fold a mirror-paired residue and its image into one unit.
    let mut units: Vec<(f64, Vec<f64>, usize)> = Vec::new();
    for (residue, members) in clusters {
        let (key, expected_len) = if meta.mirror && !is_axis_residue(residue, step) {
            let mirror_residue = (step - residue).rem_euclid(step);
            (residue.min(mirror_residue), symmetry_order as usize * 2)
        } else {
            (residue, symmetry_order as usize)
        };
        match units
            .iter_mut()
            .find(|(k, _, _)| ring_distance(*k, key, step) < INDEX_TOLERANCE * 4.0)
        {
            Some((_, existing, _)) => existing.extend(members),
            None => units.push((key, members, expected_len)),
        }
    }

    units.sort_by(|a, b| a.0.total_cmp(&b.0));
    units
        .into_iter()
        .map(|(_, mut members, expected_len)| {
            members.sort_by(f64::total_cmp);
            OrbitUnit {
                members,
                expected_len,
            }
        })
        .collect()
}

/// Every index-wheel position `position`'s orbit *should* occupy under
/// `symmetry_order`/`mirror`/`gear_teeth_abs`, independent of what any
/// tier's `indices` currently holds -- used by
/// [`crate::design::Design::add_orbit_member`] to expand one requested
/// position into its whole clean orbit before it is ever written into a
/// tier.
pub(super) fn expected_orbit(
    position: f64,
    symmetry_order: u32,
    mirror: bool,
    gear_teeth_abs: u32,
) -> Vec<f64> {
    if symmetry_order == 0 || gear_teeth_abs == 0 {
        return vec![position];
    }
    let step = f64::from(gear_teeth_abs) / f64::from(symmetry_order);
    let gear = f64::from(gear_teeth_abs);
    let mut positions: Vec<f64> = (0..symmetry_order)
        .map(|k| f64::from(k).mul_add(step, position).rem_euclid(gear))
        .collect();
    if mirror {
        let mirrored_base = (-position).rem_euclid(gear);
        for k in 0..symmetry_order {
            let m = f64::from(k).mul_add(step, mirrored_base).rem_euclid(gear);
            if !positions
                .iter()
                .any(|&p| ring_distance(p, m, gear) < INDEX_TOLERANCE)
            {
                positions.push(m);
            }
        }
    }
    positions.sort_by(f64::total_cmp);
    positions.dedup_by(|a, b| ring_distance(*a, *b, gear) < INDEX_TOLERANCE);
    // `dedup_by` only ever compares sorted-adjacent elements, so it can
    // never see the one wraparound pair that matters most here: a
    // position pinned at (or very near) `0` and one at `gear - epsilon`
    // land at opposite ends of the sorted `Vec`, not next to each other,
    // even though `ring_distance` (and the geometry) call them the same
    // tooth. Fold that last pair in separately.
    if positions.len() > 1 {
        let first = positions[0];
        let last = *positions.last().expect("len > 1 checked above");
        if ring_distance(first, last, gear) < INDEX_TOLERANCE {
            positions.pop();
        }
    }
    positions
}
