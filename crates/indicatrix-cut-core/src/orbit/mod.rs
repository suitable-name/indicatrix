//! Deriving a tier's *orbit* and editing through that derivation.
//!
//! An orbit is the index-wheel positions one physical facet occupies once a
//! schedule's `symmetry_order`/`mirror` finish rotating (and, if mirrored,
//! reflecting) it around the index gear -- computed from a tier's raw
//! `indices` list, never stored separately from it.
//!
//! # The measurement this module is built from
//!
//! Do real designs' tier index lists actually form clean orbits under their own
//! stated `symmetry_order`/`mirror`/`gear_teeth_abs`, or is that relationship a
//! fiction? Sampling 600 of the catalogue's 2,881 distinct designs (every 4th
//! design's first attached `.asc`) and classifying all 10,006 resulting tiers
//! against the model this module implements:
//!
//! | tier classification | count | share |
//! |---|---|---|
//! | `exact` (one orbit unit, fully populated) | 7,902 | 79.0% |
//! | `clean_fold` (>=2 orbit units, ALL fully populated) | 930 | 9.3% |
//! | `single` (<=1 index -- a table/culet, trivially its own orbit) | 887 | 8.9% |
//! | `mixed_fold` (>=2 orbit units, >=1 incomplete) | 179 | 1.8% |
//! | `partial` (one orbit unit, incompletely populated) | 108 | 1.1% |
//!
//! Per design, in the same 600-design sample: 48.2% are entirely `exact`/
//! `single` tiers, 43.8% have at least one `clean_fold` tier but nothing
//! worse, 5.0% have at least one `mixed_fold` tier, and 2.8% have at worst a
//! `partial` tier. That's **92.0% of designs fully consistent** with an
//! orbit model derived purely from `symmetry_order`/`mirror`/
//! `gear_teeth_abs` -- and the remaining 7.8% split cleanly into "this
//! specific occurrence never got its symmetric partner" (`partial`, benign,
//! common on a design's very first or very last facet) rather than
//! "the operator does not apply here" (`mixed_fold`, genuinely inconsistent
//! data this module must never paper over).
//!
//! # Why "unrelated" turned out to mean two different things
//!
//! A tier whose `indices` don't obviously match `symmetry_order` might really be
//! **several genuine facets folded into one tier record** -- `.asc`'s own format
//! allows a tier to fold more than one `n <name>` group together when they share
//! an angle and mast (see `indicatrix_formats::asc`'s module docs), and nothing stops two
//! *unrelated* facets from being folded the same way. Both turned out to be real,
//! in almost exactly the proportion above:
//!
//! - `clean_fold` (9.3% of tiers, e.g. `sym=4 mirror=false gear=96`,
//!   indices `96 88 80 72 64 56 48 40 32 24 16 8` splitting into three
//!   *complete* 4-member orbit units) is exactly the "structurally sound at
//!   finer granularity" case: several facets, each individually a clean
//!   orbit under the *same* stated symmetry, written as one tier because
//!   `GemCAD` had no reason to split them. Because a `ConstraintTier`'s
//!   `angle_deg`/`constraint` already apply to every member of `indices`
//!   uniformly, this case needs **no special edit-time handling at all**:
//!   `Edit::ModifyTier`/`Edit::SetConstraint` are already orbit-consistent
//!   by construction here, since all the folded facets share the tier's one
//!   angle and mast in the source file itself. The only place granularity
//!   matters is *membership* (adding/removing one occurrence), which is
//!   what [`crate::design::Design::add_orbit_member`]/
//!   [`crate::design::Design::remove_orbit_member`] operate on -- see below.
//! - `mixed_fold` and `partial` together (2.9% of tiers) are the genuine
//!   "does not cleanly apply" residue -- a `partial` orbit unit (e.g. `sym=3
//!   mirror=true gear=96`, indices `[30, 66]` against an expected 6) is
//!   usually just a design missing one printed continuation line or a
//!   facet at the very edge of the schedule; `mixed_fold` (e.g. `sym=4
//!   mirror=true gear=96`, indices `[88, 72, 56, 40, 24, 8]` splitting into
//!   an axis unit with 2 of an expected 4 members and a generic mirror-pair
//!   unit with 4 of an expected 8) is real incoherence -- units present, but
//!   neither one complete, so there is no single clean orbit hiding under
//!   the stated symmetry either. [`orbit_units`] reports both exactly as
//!   measured (an `OrbitUnit` with `members.len() < expected_len`); nothing
//!   in this crate ever rewrites a design's `indices` to "fix" one -- a
//!   non-orbit design must stay editable and must never be silently
//!   corrected into symmetry its author never wrote.
//!
//! # The orbit model
//!
//! A facet at azimuth `base` under `symmetry_order`-fold rotation repeats at
//! `base + k*step` for `k = 0..symmetry_order`, where
//! `step = gear_teeth_abs / symmetry_order`; every member of that rotational
//! orbit shares the same residue `base mod step`. When `mirror` is set, the
//! facet's mirror image sits at azimuth `-base mod gear_teeth_abs`, which
//! lands on a *different* residue unless `base` is already on the mirror
//! axis (residue `0` or `step/2`, where the mirror maps the rotational
//! orbit onto itself). So one facet's complete orbit is:
//! - one residue cluster of `symmetry_order` members, if `base` is on-axis
//!   or the schedule isn't mirrored, or
//! - two residue clusters of `symmetry_order` members each (`base`'s own
//!   cluster and its mirror image's), if `mirror` is set and `base` is
//!   off-axis.
//!
//! [`orbit_units`] groups a tier's raw `indices` into exactly these units;
//! [`OrbitUnit::is_complete`] is the completeness check the table above
//! measures at corpus scale.
//!
//! # Module layout
//!
//! [`model`] is the pure orbit math (`OrbitUnit`/`orbit_units`/`expected_orbit`);
//! [`edit`] is [`crate::design::Design`]'s own orbit-editing methods built on top
//! of it.

mod edit;
mod model;
#[cfg(test)]
mod tests;

pub use model::{INDEX_TOLERANCE, OrbitUnit, mirror_indices, orbit_units, rotate_indices};
