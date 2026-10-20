//! Labels one spectral sample by what its hero path did, with the metrics' own predicates.
//!
//! The spectral renderer's [`PathEvents`] record says where a path entered and left the
//! stone; this module turns that into the same words the analytical map uses (windowed,
//! returned, extinct with a reason) so the overlay can show either source in one palette.
//! It lives inside `color::metrics` because the visibility test and the lighting verdict
//! ([`returned_or_reason`], [`ExitLighting`]) are private to it: the label is decided by the
//! very code the analytical classifier runs, with the same thresholds (`y < -0.05` for a
//! pavilion exit, `y > 0.05` for an upward one).

use glam::Vec3;

use super::{
    image_types::ExtinctionReason, lighting::ExitLighting, visibility::returned_or_reason,
};
use crate::optics::raytracer::{EnvironmentSource, PathEvents, PathTermination};

/// Version of the spectral attribution labelling; shown in the legend and in export notes.
pub const SPECTRAL_ATTRIBUTION_VERSION: u32 = 1;

/// Outward-normal `y` below which an exit is out through the pavilion (windowing); the
/// analytical tracer's predicate.
const WINDOW_NORMAL_Y: f32 = -0.05;

/// Exit-direction `y` above which an exit heads back towards the observer's hemisphere.
const UPWARD_DIR_Y: f32 = 0.05;

/// What one spectral sample did, as a class of the overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpectralClass {
    /// The first facet event reflected the ray off the outside of the stone.
    SurfaceGlare,
    /// The first exit left through a pavilion facet (outward normal pointing down).
    Windowed,
    /// The first exit went upward, outside the head-shadow cone, towards a light.
    Returned,
    /// Everything else, with the reason.
    Extinct(ExtinctionReason),
}

/// The lighting verdict the analytical map uses, built once per environment.
///
/// Building it probes the environment (96 directions); share one instance across a frame's
/// samples.
#[derive(Clone)]
pub struct AttributionLighting<'a>(ExitLighting<'a>);

impl<'a> AttributionLighting<'a> {
    /// The lighting of `environment`, exactly as `evaluate_optical_image` builds it.
    #[must_use]
    pub fn new(environment: EnvironmentSource<'a>) -> Self {
        Self(ExitLighting::new(environment))
    }
}

/// The reason a path that never made an exit stopped.
const fn unexited_reason(termination: Option<PathTermination>) -> ExtinctionReason {
    match termination {
        Some(PathTermination::RussianRoulette) => ExtinctionReason::RussianRoulette,
        Some(PathTermination::ScatterAbsorbed) => ExtinctionReason::ScatterAbsorbed,
        Some(PathTermination::HitCap) => ExtinctionReason::BounceBudget,
        // Inside the stone every ray meets a facet, so leaving without an exit event is the
        // analytical map's "no intersection" (a concave stone's cavity can do it).
        Some(PathTermination::Escaped) => ExtinctionReason::NoIntersection,
        None => ExtinctionReason::Invalid,
    }
}

/// The class of the path `events` describes.
///
/// The path is seen by an observer in unit direction `observer` from the stone (the reverse
/// of the camera ray); `None` when the camera ray never met the stone (background or
/// backdrop).
#[must_use]
pub fn classify_path(
    events: &PathEvents,
    observer: Vec3,
    lighting: &AttributionLighting<'_>,
) -> Option<SpectralClass> {
    let (_, entered) = events.first_facet?;
    if !entered {
        return Some(SpectralClass::SurfaceGlare);
    }
    let Some(exit) = events.first_exit else {
        return Some(SpectralClass::Extinct(unexited_reason(events.termination)));
    };
    Some(if exit.normal.y < WINDOW_NORMAL_Y {
        SpectralClass::Windowed
    } else if exit.dir.y > UPWARD_DIR_Y {
        match returned_or_reason(exit.dir, observer, &lighting.0) {
            Ok(()) => SpectralClass::Returned,
            Err(reason) => SpectralClass::Extinct(reason),
        }
    } else {
        SpectralClass::Extinct(ExtinctionReason::SideExit)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optics::raytracer::{ExitEvent, LightingPreset};

    fn lighting() -> AttributionLighting<'static> {
        AttributionLighting::new(LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95))
    }

    fn exiting(normal: Vec3, dir: Vec3) -> PathEvents {
        PathEvents {
            first_facet: Some((0, true)),
            first_exit: Some(ExitEvent {
                facet: 1,
                normal,
                dir: dir.normalize(),
                bounce: 2,
            }),
            termination: Some(PathTermination::Escaped),
            ..PathEvents::default()
        }
    }

    #[test]
    fn a_path_with_no_facet_event_is_a_miss() {
        let class = classify_path(&PathEvents::default(), Vec3::Z, &lighting());
        assert_eq!(class, None);
    }

    #[test]
    fn an_outside_reflection_is_glare_whatever_happened_after() {
        let events = PathEvents {
            first_facet: Some((4, false)),
            termination: Some(PathTermination::Escaped),
            ..PathEvents::default()
        };
        assert_eq!(
            classify_path(&events, Vec3::Z, &lighting()),
            Some(SpectralClass::SurfaceGlare)
        );
    }

    #[test]
    fn exits_follow_the_analytical_predicates() {
        let light = lighting();
        // Out through a facet facing down: windowed, even though the ray heads upward.
        let windowed = exiting(-Vec3::Y, Vec3::new(0.1, 1.0, 0.0));
        assert_eq!(
            classify_path(&windowed, Vec3::Z, &light),
            Some(SpectralClass::Windowed)
        );
        // Upward out of a crown facet, away from the eye: returned under a uniform dome.
        let returned = exiting(Vec3::Y, Vec3::new(0.3, 0.9, 0.0));
        assert_eq!(
            classify_path(&returned, Vec3::Z, &light),
            Some(SpectralClass::Returned)
        );
        // The same exit straight at the eye is lost to the observer's own shadow.
        let dir = returned.first_exit.expect("exit").dir;
        assert_eq!(
            classify_path(&returned, dir, &light),
            Some(SpectralClass::Extinct(ExtinctionReason::ObserverShadow))
        );
        // Neither down nor up: a side exit.
        let side = exiting(Vec3::X, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(
            classify_path(&side, Vec3::Z, &light),
            Some(SpectralClass::Extinct(ExtinctionReason::SideExit))
        );
    }

    #[test]
    fn paths_that_never_exit_are_labelled_by_how_they_stopped() {
        let light = lighting();
        let stopped = |termination| {
            let events = PathEvents {
                first_facet: Some((0, true)),
                termination,
                ..PathEvents::default()
            };
            classify_path(&events, Vec3::Z, &light)
        };
        let extinct = |reason| Some(SpectralClass::Extinct(reason));
        assert_eq!(
            stopped(Some(PathTermination::HitCap)),
            extinct(ExtinctionReason::BounceBudget)
        );
        assert_eq!(
            stopped(Some(PathTermination::RussianRoulette)),
            extinct(ExtinctionReason::RussianRoulette)
        );
        assert_eq!(
            stopped(Some(PathTermination::ScatterAbsorbed)),
            extinct(ExtinctionReason::ScatterAbsorbed)
        );
        assert_eq!(
            stopped(Some(PathTermination::Escaped)),
            extinct(ExtinctionReason::NoIntersection)
        );
        assert_eq!(stopped(None), extinct(ExtinctionReason::Invalid));
    }
}
