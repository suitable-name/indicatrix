//! The fingerprint stored beside each cached catalogue artefact (previews and tilt
//! curves), naming what it was computed with.

use super::{PREVIEW_LIGHT_PITCH, PREVIEW_LIGHT_YAW, PREVIEW_LIGHTING_PRESET};

/// Which cached catalogue artefact a [`cache_fingerprint`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheKind {
    /// The front/top preview PNGs (`Database::save_preview_images`): `size` x `size`
    /// pixels, `spp` samples per pixel, at most `max_bounces` bounces per path.
    Preview {
        /// Square render dimension in pixels.
        size: u32,
        /// Samples per pixel.
        spp: u32,
        /// Bounce cap of the render.
        max_bounces: u32,
    },
    /// The tilt-performance curves (`Database::save_tilt_curves`): a fixed sweep with
    /// no image size or sample budget of its own.
    TiltCurves,
}

/// The fingerprint stored beside a cached catalogue artefact: a stable string naming
/// everything that decides what the artefact looks like.
///
/// It lives in `diagram_previews.params_fingerprint` / `diagram_tilt_curves.params_fingerprint`,
/// so `Database::entry_ids_missing_previews` / `entry_ids_missing_tilt_curves` can tell a
/// current artefact from one made by a different renderer or with different settings.
///
/// It carries the tracer's [`indicatrix::BUILD_ID`] (bumped with any change to what the
/// tracer produces), the `kind`'s own parameters (size, samples and bounce cap for a
/// preview), the lighting (the preset's label for a preview, and the light pose both
/// artefacts share) and the material the design was rendered in. The same function
/// serves both batches, so the two can never drift in which parameters they track; it is
/// pure and stable across runs, which is what a stored comparison needs.
///
/// `material_name` is the design's persisted preview material (`None` before one has
/// been picked).
#[must_use]
pub fn cache_fingerprint(kind: CacheKind, material_name: Option<&str>) -> String {
    let tracer = indicatrix::BUILD_ID;
    let material = material_name.unwrap_or("none");
    let light = format!("{PREVIEW_LIGHT_YAW}/{PREVIEW_LIGHT_PITCH}");
    match kind {
        CacheKind::Preview {
            size,
            spp,
            max_bounces,
        } => format!(
            "preview;tracer={tracer};size={size};spp={spp};bounces={max_bounces};\
             lighting={};light={light};material={material}",
            PREVIEW_LIGHTING_PRESET.label()
        ),
        CacheKind::TiltCurves => format!("tilt;tracer={tracer};light={light};material={material}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREVIEW: CacheKind = CacheKind::Preview {
        size: 160,
        spp: 256,
        max_bounces: 12,
    };

    #[test]
    fn cache_fingerprint_is_stable_and_names_the_tracer_build() {
        let fingerprint = cache_fingerprint(PREVIEW, Some("Diamond"));
        assert_eq!(fingerprint, cache_fingerprint(PREVIEW, Some("Diamond")));
        assert!(fingerprint.contains(indicatrix::BUILD_ID), "{fingerprint}");
        assert!(
            cache_fingerprint(CacheKind::TiltCurves, Some("Diamond"))
                .contains(indicatrix::BUILD_ID)
        );
    }

    #[test]
    fn cache_fingerprint_changes_with_every_parameter_it_tracks() {
        let base = cache_fingerprint(PREVIEW, Some("Diamond"));
        let other_size = CacheKind::Preview {
            size: 128,
            spp: 256,
            max_bounces: 12,
        };
        let other_spp = CacheKind::Preview {
            size: 160,
            spp: 64,
            max_bounces: 12,
        };
        let other_bounces = CacheKind::Preview {
            size: 160,
            spp: 256,
            max_bounces: 8,
        };
        for changed in [
            cache_fingerprint(other_size, Some("Diamond")),
            cache_fingerprint(other_spp, Some("Diamond")),
            cache_fingerprint(other_bounces, Some("Diamond")),
            cache_fingerprint(PREVIEW, Some("Sapphire")),
            cache_fingerprint(PREVIEW, None),
            cache_fingerprint(CacheKind::TiltCurves, Some("Diamond")),
        ] {
            assert_ne!(changed, base);
        }
        assert!(base.contains(PREVIEW_LIGHTING_PRESET.label()), "{base}");
    }

    /// The tilt sweep has no image size or sample budget, so changing the preview
    /// settings must not look like a reason to recompute every design's curves.
    #[test]
    fn the_tilt_fingerprint_names_no_preview_setting() {
        let fingerprint = cache_fingerprint(CacheKind::TiltCurves, Some("Diamond"));
        for setting in ["size=", "spp=", "bounces="] {
            assert!(!fingerprint.contains(setting), "{fingerprint}");
        }
    }
}
