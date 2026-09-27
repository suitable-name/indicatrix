//! Reader for Gem Cut Studio's `.gcs` design format.
//!
//! Gem Cut Studio ("GCS") is a Windows faceting-design application distinct from
//! `GemCAD`; this module is not produced, endorsed, or affiliated with Gem Cut
//! Studio or its author (see the crate-level docs for the full affiliation note).
//! It exists for the same reason [`crate::asc`] exists: some real-world designs in
//! the wild are only ever published as a `.gcs` file, with no `.asc` counterpart,
//! and reading one shouldn't require Gem Cut Studio itself.
//!
//! # Format (reverse-engineered from a real-world corpus of 59 `.gcs` files, 56 of
//! which have a sibling `.asc` for the same design -- see "Verification" below)
//!
//! A `.gcs` file is plain-text XML (no prolog, no external DTD, no namespaces) with
//! one attribute-only root wrapping a flat, non-recursive element tree:
//!
//! ```text
//! <GemCutStudio version="1000">
//!     <index gear="64" base="0" symmetry="4" mirror="0"/>
//!     <tier angle="126.39" depth="0.6664" name="P1" instructions="" visible="true" guide="false">
//!         <facet nx="-0" ny="-0.805" nz="-0.593" index_angle="0">
//!             <vertex x="-0.0827" y="-0.8186" z="-0.0126"/>
//!             ...
//!         </facet>
//!         ...
//!     </tier>
//!     ...
//!     <render material="176 Corundum" refractive_index="1.76" dispersion="0.018" clarity="100" density="1.4" lighting_model="Random">
//!         <color r="0.71" g="0.73" b="0.95"/>
//!     </render>
//!     <info title="..." author="..." date="..." ri_min="1.7" ri_max="2.15" shape="Octagon" footer1="..." footer2="..."/>
//! </GemCutStudio>
//! ```
//!
//! Unlike [`crate::asc`]'s `.asc` (a cutting *schedule*: angle, mast, and index
//! positions the cutter still has to solve into a shape), a `.gcs` file is a
//! *solved* design: every `<tier>` already carries its facets as closed polygons
//! (`<facet>` children, each a list of real `<vertex>` points) on a stone
//! normalized so its own reference plane sits at radius 1. That difference in kind
//! -- schedule vs. solved geometry -- is exactly why `depth` and `.asc`'s `mast` are
//! **not interchangeable** even though both nominally mean "how far this facet
//! plane sits from center"; see "What does not carry over" below.
//!
//! ## What is confirmed, and how
//!
//! Every field below was checked against the 56 real `.gcs`/`.asc` sibling pairs in
//! the corpus (`facet_diagrams.sqlite`'s `attached_files` table, joined on
//! `detail_id`), using [`crate::asc::parse_asc`] on the `.asc` side as ground
//! truth. 44 of the 56 pairs (79%) matched on every check below with zero
//! discrepancy; the remaining 12 are understood corpus-data edge cases, not
//! parser gaps (see "Known discrepancies").
//!
//! - **`<index gear>`** matches `.asc`'s `g` line tooth count exactly (as a
//!   magnitude -- `.asc` occasionally signs it for handedness, `.gcs` never does).
//! - **[`GcsTier::angle_deg`]** uses a *different convention* from
//!   [`crate::asc::AscTier::angle_deg`], but a fully verified one: `.gcs` measures
//!   a single continuous polar angle from the crown apex/table-normal direction
//!   (`0`) through the girdle plane (`90`, vertical) to the culet direction
//!   (`180`), rather than `.asc`'s signed "negative = pavilion, non-negative =
//!   crown" split. For every crown tier (`.asc` angle `>= 0`) the two values are
//!   *identical*; for every pavilion tier (`.asc` angle `< 0`) `.gcs`'s angle
//!   equals `180.0 + asc_angle` to within float32 rounding (`.gcs` stores its
//!   trigonometric fields at `f32` precision even though the XML prints them as
//!   `f64`-width decimals -- hence the "give or take a few times `1e-5`" residue
//!   seen when cross-checking). [`GcsTier::to_signed_asc_angle`] applies this
//!   verified transform.
//! - **`<facet index_angle>`** matches `.asc`'s tooth-number indices exactly once
//!   converted to degrees: `index_angle = (tooth % gear) / gear * 360`. Checked
//!   facet-by-facet (not just tier-by-tier) across all 44 clean-passing pairs.
//! - **`<render refractive_index>`** matches `.asc`'s `I` line in every pair where
//!   the two files actually describe the same material (see "Known
//!   discrepancies" for the three that do not).
//! - Total facet-plane count (summed across every tier) matches `.asc`'s summed
//!   index count exactly in every clean-passing pair.
//!
//! ## What does not carry over
//!
//! - **[`GcsTier::depth`] is not `.asc`'s `mast`.** Comparing matched tiers within
//!   a single real design (`attached_files` id 1124, detail 553, "Octabar-X") shows
//!   `depth / mast` ranging smoothly from `0.90` (at the girdle, angle 90) up
//!   through `1.18` (at the table, angle 0) -- not a constant, and not a simple
//!   trig function of the tier's own angle either (the pavilion tiers of the same
//!   file give a completely different, non-overlapping ratio curve from the crown
//!   tiers). This module does not guess at a conversion. Reconciling the two
//!   requires the same full meet-point geometry solve `.asc`'s own mast values are
//!   solved from -- exactly the job of `indicatrix::geometry::meet_solver`, not
//!   this crate.
//! - **`<index base>`** is carried through as-is (presumably analogous to `.asc`'s
//!   `g` line reference-angle field) but every sample in the corpus has `base="0"`,
//!   so this module has no real, non-zero example to verify that analogy against.
//!   Treat it as unconfirmed.
//! - **`<index symmetry>` and `<index mirror>` do not reliably mirror `.asc`'s `y`
//!   line.** Cross-checking all 56 pairs: `symmetry` sometimes matches `.asc`'s
//!   symmetry order exactly, but is very often `1` even when the design's real
//!   rotational symmetry (per its own `.asc`) is 4, 8, or 16 -- `.gcs` appears to
//!   fully unroll the facet list (no compression via the index/symmetry mechanism)
//!   for many designs and not others, and this module could not determine the
//!   rule that decides which. `mirror` is stranger still: real corpus values
//!   include `0`, `1`, `2`, `3`, `4`, and `5`, which rules out a simple boolean
//!   matching `.asc`'s `y`/`n` mirror flag. Both fields are kept as raw integers
//!   ([`GcsIndex::symmetry`], [`GcsIndex::mirror`]) with no derived
//!   `is_mirrored()`-style helper, specifically so callers don't inherit a
//!   boolean assumption this module cannot back up.
//!
//! ## Known discrepancies (12 of 56 pairs)
//!
//! - **Flat, single-facet tiers (the table, and occasionally the culet) have an
//!   arbitrary `index_angle`.** A tier spanning the entire top or bottom of the
//!   stone has only one facet and no real azimuthal position, so `.gcs` appears to
//!   just pick something (observed: `0`, `11.25`, `22.5`, `45` across different
//!   files) rather than echo the design's own tooth number for that facet. 7 of
//!   the 12 discrepant pairs are exactly this.
//! - **A few "cut corner" shapes (2 of 56) merge tiers differently than this
//!   module's `.asc`-side grouping expects** when the design has more than one
//!   facet sharing an angle+mast at the `.asc` level; the exact grouping rule
//!   `.gcs` uses there was not pinned down.
//! - **3 of 56 pairs have a genuine refractive-index mismatch** between the
//!   `.gcs` and its catalogued `.asc` sibling (e.g. 1.76 vs. 1.54) -- almost
//!   certainly two different material variants of the same design filed under one
//!   catalog entry, not a parsing issue.
//!
//! None of these were "fixed" by loosening a check; they are reported here so a
//! caller knows exactly which 21% of real files might disagree with an `.asc`
//! sibling and why.
//!
//! # What this module does not attempt
//!
//! There is no `.gcs` *writer*: nothing downstream in this workspace produces
//! `.gcs` files (the editor's own save format is `.indicatrix.toml`, see
//! [`crate::native`], and its export path targets `.asc`), so a serializer would
//! have no real caller and no way to be verified against anything.

mod design;
mod error;
mod metadata;
mod parse;
mod tier;
mod tokenize;

#[cfg(test)]
mod tests;

pub use design::GcsDesign;
pub use error::GcsParseError;
pub use metadata::{GcsColor, GcsIndex, GcsInfo, GcsRender};
pub use parse::parse_gcs;
pub use tier::{GcsFacet, GcsTier, GcsVertex};
