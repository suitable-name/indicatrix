//! The attribution recorder's hook points on the bounce loop's [`PathState`].
//!
//! A child module of `bounce_body` so the hooks read the loop's private state; each one is a
//! no-op while `events` is `None` (every plain entry point) and none of them writes anything
//! the trace reads, so the traced arithmetic is untouched.

use super::PathState;
use crate::optics::raytracer::{camera::HitRecord, scattering::ScatterStepOutcome};

impl PathState<'_> {
    /// After a facet event: `inside_before` is the geometric truth read just before the
    /// reflect/refract dispatch, so the recorder sees the side flag the intersection settled.
    pub(super) fn record_facet(&mut self, bounce: u32, hit: HitRecord, inside_before: bool) {
        let (inside_after, dir_after) = (self.inside_gem, self.current_ray.dir);
        let fluoresced = self.lambdas != self.camera_lambdas;
        if let Some(events) = self.events.as_deref_mut() {
            events.facet(bounce, hit, inside_before, inside_after, dir_after);
            if fluoresced {
                events.fluoresce();
            }
        }
    }

    /// After the ray missed the stone (the escape block ran).
    pub(super) fn record_miss(&mut self, bounce: u32) {
        let dir = self.current_ray.dir;
        let fluoresced = self.lambdas != self.camera_lambdas;
        if let Some(events) = self.events.as_deref_mut() {
            events.miss(bounce, dir);
            if fluoresced {
                events.fluoresce();
            }
        }
    }

    /// After the fluorescence vertex moved the path to the excitation wavelength. Called at the
    /// vertex itself, so a path the same segment's scatter step then absorbs, or a vertex whose
    /// excitation wavelength equals the camera's, still counts.
    pub(super) fn record_fluoresce(&mut self) {
        if let Some(events) = self.events.as_deref_mut() {
            events.fluoresce();
        }
    }

    /// After the scattering step drew its outcome.
    pub(super) fn record_scatter(&mut self, outcome: &ScatterStepOutcome) {
        if let Some(events) = self.events.as_deref_mut()
            && matches!(
                outcome,
                ScatterStepOutcome::ScatteredAndSurvived(_)
                    | ScatterStepOutcome::ScatteredAndTerminated
            )
        {
            events.scatter();
        }
    }
}
