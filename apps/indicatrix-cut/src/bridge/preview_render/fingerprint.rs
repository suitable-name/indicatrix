//! The fingerprint stored beside each cached catalogue artefact (previews and tilt
//! curves), naming what it was computed with.

use super::{
    BATCH_TILT_LIGHTING_PRESET, PREVIEW_LIGHT_PITCH, PREVIEW_LIGHT_YAW, PREVIEW_LIGHTING_PRESET,
};

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
    /// The fast solid-renderer stand-in picture (`super::render_view_solid`): `size` x
    /// `size` pixels, no samples. Never equal to a [`Self::Preview`] fingerprint, so a
    /// design that only has this picture still counts as missing its traced preview.
    SolidDraft {
        /// Square render dimension in pixels.
        size: u32,
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
/// preview), the lighting (the preset's label for each artefact -- the preview rig or the
/// tilt scoring preset -- and the light pose both share) and the material the design was rendered in. The same function
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
        CacheKind::SolidDraft { size } => format!("solid-draft;size={size}"),
        CacheKind::TiltCurves => format!(
            "tilt;tracer={tracer};lighting={};light={light};material={material}",
            BATCH_TILT_LIGHTING_PRESET.label()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::optics::raytracer::LightingPreset;

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

    /// The tilt fingerprint names the scoring lighting preset by its stable label, so
    /// curves stored under another preset (the ring lights before 2026-10-07) are stale.
    #[test]
    fn the_tilt_fingerprint_names_the_scoring_lighting_preset() {
        let fingerprint = cache_fingerprint(CacheKind::TiltCurves, Some("Diamond"));
        assert_eq!(
            fingerprint,
            format!(
                "tilt;tracer={};lighting={};light={PREVIEW_LIGHT_YAW}/{PREVIEW_LIGHT_PITCH};\
                 material=Diamond",
                indicatrix::BUILD_ID,
                BATCH_TILT_LIGHTING_PRESET.label()
            )
        );
        assert_ne!(
            BATCH_TILT_LIGHTING_PRESET.label(),
            LightingPreset::RingLights.label()
        );
        let old_ring_lights = fingerprint.replace(
            BATCH_TILT_LIGHTING_PRESET.label(),
            LightingPreset::RingLights.label(),
        );
        assert_ne!(fingerprint, old_ring_lights);
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
