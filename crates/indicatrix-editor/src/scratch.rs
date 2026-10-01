//! Scratch-field change detection.
//!
//! Which design-derived groups (material, gear, symmetry, preform, girdle diameter,
//! header/footnote meta) actually changed since a UI last seeded its editable form fields
//! from the design.
//!
//! A refresh that re-seeded every form field from the design unconditionally would
//! silently overwrite whatever the user is mid-typing in an unrelated field (Apply
//! Gear blanking an in-progress Preform Half-Width, say). A UI keeps one
//! [`PushedScratch`] per design and calls [`PushedScratch::record`] exactly once per
//! refresh; each [`ScratchDelta`] flag gates its own group's re-seed.

use indicatrix_cut_core::{Design, MaterialSelection, PreformSpec};

/// The last-pushed value of every design-derived group a form mirrors.
///
/// Every field
/// starts unobserved, so the very first refresh after New/Load reports every group
/// changed (a correct, if slightly redundant, initial push).
#[derive(Default)]
pub struct PushedScratch {
    material: Option<MaterialSelection>,
    gear_teeth: Option<i32>,
    symmetry: Option<(u32, bool)>,
    preform: Option<PreformSpec>,
    girdle_diameter_mm: GirdleDiameterPush,
    /// `(headers, footnotes, gear_reference_angle)` -- see [`ScratchDelta::meta`].
    meta: Option<(Vec<String>, Vec<String>, f64)>,
}

/// [`PushedScratch`]'s girdle-diameter slot -- not a plain `Option<Option<f64>>`,
/// since the design's own value is already an `Option<f64>` (unset vs a real
/// dimension): nesting it would conflate "unset" with "never observed".
#[derive(Clone, Copy, Default, PartialEq)]
enum GirdleDiameterPush {
    /// No push recorded yet.
    #[default]
    Unobserved,
    /// The last-pushed value; `None` when the design has no girdle diameter set.
    Observed(Option<f64>),
}

/// Which of [`PushedScratch`]'s groups changed since the last push -- six
/// independent flags, so a change to one group never re-seeds (and so discards an
/// in-progress edit in) another.
#[derive(Debug, PartialEq, Eq)]
pub struct ScratchDelta {
    /// The material selection changed.
    pub material: bool,
    /// The gear tooth count changed.
    pub gear: bool,
    /// The symmetry order or mirror flag changed.
    pub symmetry: bool,
    /// The preform changed.
    pub preform: bool,
    /// The girdle diameter changed.
    pub girdle: bool,
    /// The design's headers/footnotes/gear-reference angle changed.
    pub meta: bool,
}

impl PushedScratch {
    /// Compares `design`'s current settings/preform/yield-relevant fields against the
    /// snapshot recorded the last time this ran, records a fresh snapshot, and
    /// returns which groups changed. Call exactly once per refresh -- calling it from
    /// inside a function a refresh also calls would compare a later group against a
    /// snapshot an earlier group of the same refresh already overwrote.
    pub fn record(&mut self, design: &Design) -> ScratchDelta {
        let meta_now = (
            design.meta.headers.clone(),
            design.meta.footnotes.clone(),
            design.meta.gear_reference_angle,
        );
        let delta = ScratchDelta {
            material: self.material.as_ref() != Some(&design.material),
            gear: self.gear_teeth != Some(design.meta.gear_teeth),
            symmetry: self.symmetry != Some((design.meta.symmetry_order, design.meta.mirror)),
            preform: self.preform != Some(design.preform),
            girdle: self.girdle_diameter_mm
                != GirdleDiameterPush::Observed(design.girdle_diameter_mm),
            meta: self.meta.as_ref() != Some(&meta_now),
        };
        *self = Self {
            material: Some(design.material.clone()),
            gear_teeth: Some(design.meta.gear_teeth),
            symmetry: Some((design.meta.symmetry_order, design.meta.mirror)),
            preform: Some(design.preform),
            girdle_diameter_mm: GirdleDiameterPush::Observed(design.girdle_diameter_mm),
            meta: Some(meta_now),
        };
        delta
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_push_reports_everything_and_a_repeat_reports_nothing() {
        let design = crate::EditorSession::fresh().design;
        let mut scratch = PushedScratch::default();
        let first = scratch.record(&design);
        assert!(first.material && first.gear && first.symmetry);
        assert!(first.preform && first.girdle && first.meta);
        let again = scratch.record(&design);
        assert!(!(again.material || again.gear || again.symmetry));
        assert!(!(again.preform || again.girdle || again.meta));
    }

    #[test]
    fn only_the_changed_group_is_reported() {
        let mut design = crate::EditorSession::fresh().design;
        let mut scratch = PushedScratch::default();
        scratch.record(&design);
        design.girdle_diameter_mm = Some(6.5);
        let delta = scratch.record(&design);
        assert!(delta.girdle);
        assert!(!(delta.material || delta.gear || delta.symmetry || delta.preform || delta.meta));
    }
}
