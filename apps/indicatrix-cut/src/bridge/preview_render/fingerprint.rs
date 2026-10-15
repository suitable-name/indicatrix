//! The fingerprint stored beside each cached catalogue artefact (previews and tilt
//! curves), naming what it was computed with.

use super::{
    BATCH_TILT_LIGHTING_PRESET, PREVIEW_LIGHT_PITCH, PREVIEW_LIGHT_YAW, PREVIEW_LIGHTING_PRESET,
};
use indicatrix_vault::model::preview::{PreviewMaterialSource, PreviewMaterialState};
use std::fmt::Write as _;

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

/// What a fingerprint says about the material a design was rendered or swept in: the
/// persisted preview material, the stone size it was sized with and where it came from.
///
/// Built from the persisted `diagram_previews` state (`Self::from_state`) by the staleness
/// scans, and from the resolved design by the writers, so a stored fingerprint and the
/// one a scan builds for the same state are the same string.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FingerprintMaterial<'a> {
    /// The persisted material name; `None` before one has been picked.
    pub name: Option<&'a str>,
    /// The girdle diameter in millimetres the material is sized with; `0.0` (or anything
    /// that is not a finite positive number) is "not set", the 7 mm calibration.
    pub girdle_mm: f64,
    /// Whether the name and size came from the design's own file rather than from the
    /// refractive-index pick.
    pub from_design_file: bool,
}

impl<'a> FingerprintMaterial<'a> {
    /// A refractive-index pick: `name` (`None` before one has been picked), no size, not
    /// from the design file. The material part of every fingerprint written before the
    /// size and the source were tracked.
    #[cfg(test)]
    #[must_use]
    pub const fn picked(name: Option<&'a str>) -> Self {
        Self {
            name,
            girdle_mm: 0.0,
            from_design_file: false,
        }
    }

    /// The material of a persisted `diagram_previews` row (`Database::
    /// get_preview_material_state`, or the state a scan is handed).
    #[must_use]
    pub fn from_state(state: &'a PreviewMaterialState) -> Self {
        Self {
            name: state.name.as_deref(),
            girdle_mm: state.girdle_mm.unwrap_or(0.0),
            from_design_file: state.source == PreviewMaterialSource::DesignFile,
        }
    }

    /// The material part of a fingerprint: `material=<name>` (`none` before one is
    /// picked), then `;size=<girdle, two decimals>` only when a size is set and
    /// `;source=design` only when the design's file decided it, so a design with neither
    /// keeps the exact text it had before they were tracked.
    fn fingerprint_part(&self) -> String {
        let mut part = format!("material={}", self.name.unwrap_or("none"));
        if self.girdle_mm.is_finite() && self.girdle_mm > 0.0 {
            let _ = write!(part, ";size={:.2}", self.girdle_mm);
        }
        if self.from_design_file {
            part.push_str(";source=design");
        }
        part
    }
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
/// tilt scoring preset -- and the light pose both share) and the material the design was
/// rendered in. The same function serves both batches, so the two can never drift in
/// which parameters they track; it is pure and stable across runs, which is what a
/// stored comparison needs.
///
/// `material` is the design's persisted preview material ([`FingerprintMaterial`]): its
/// name (`None` before one has been picked), and -- only when the design's own file
/// decided them -- the stone size it is drawn at and the source. A design the refractive
/// index decided keeps the text it always had; a design whose file names a material or a
/// girdle diameter gets `;size=<mm>` and/or `;source=design` appended after the name
/// (so the `size=` of the stone size follows the material, never the image size of a
/// preview).
#[must_use]
pub fn cache_fingerprint(kind: CacheKind, material: FingerprintMaterial<'_>) -> String {
    let tracer = indicatrix::BUILD_ID;
    let material = material.fingerprint_part();
    let light = format!("{PREVIEW_LIGHT_YAW}/{PREVIEW_LIGHT_PITCH}");
    match kind {
        CacheKind::Preview {
            size,
            spp,
            max_bounces,
        } => format!(
            "preview;tracer={tracer};size={size};spp={spp};bounces={max_bounces};\
             lighting={};light={light};{material}",
            PREVIEW_LIGHTING_PRESET.label()
        ),
        CacheKind::SolidDraft { size } => format!("solid-draft;size={size}"),
        CacheKind::TiltCurves => format!(
            "tilt;tracer={tracer};lighting={};light={light};{material}",
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

    /// The material of a refractive-index pick, unsized.
    fn picked(name: &str) -> FingerprintMaterial<'_> {
        FingerprintMaterial::picked(Some(name))
    }

    /// The material of a design whose own file decided it.
    fn from_file(name: &str, girdle_mm: f64) -> FingerprintMaterial<'_> {
        FingerprintMaterial {
            name: Some(name),
            girdle_mm,
            from_design_file: true,
        }
    }

    #[test]
    fn cache_fingerprint_is_stable_and_names_the_tracer_build() {
        let fingerprint = cache_fingerprint(PREVIEW, picked("Diamond"));
        assert_eq!(fingerprint, cache_fingerprint(PREVIEW, picked("Diamond")));
        assert!(fingerprint.contains(indicatrix::BUILD_ID), "{fingerprint}");
        assert!(
            cache_fingerprint(CacheKind::TiltCurves, picked("Diamond"))
                .contains(indicatrix::BUILD_ID)
        );
    }

    #[test]
    fn cache_fingerprint_changes_with_every_parameter_it_tracks() {
        let base = cache_fingerprint(PREVIEW, picked("Diamond"));
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
            cache_fingerprint(other_size, picked("Diamond")),
            cache_fingerprint(other_spp, picked("Diamond")),
            cache_fingerprint(other_bounces, picked("Diamond")),
            cache_fingerprint(PREVIEW, picked("Sapphire")),
            cache_fingerprint(PREVIEW, FingerprintMaterial::picked(None)),
            cache_fingerprint(CacheKind::TiltCurves, picked("Diamond")),
            cache_fingerprint(PREVIEW, from_file("Diamond", 0.0)),
            cache_fingerprint(PREVIEW, from_file("Diamond", 9.0)),
        ] {
            assert_ne!(changed, base);
        }
        assert!(base.contains(PREVIEW_LIGHTING_PRESET.label()), "{base}");
    }

    /// A design the refractive index decided and that has no stone size keeps the text its
    /// fingerprint had before the size and the source were tracked, byte for byte, so
    /// only designs whose file names a material or a size are rendered again.
    #[test]
    fn an_unsized_refractive_index_pick_keeps_the_old_fingerprint_exactly() {
        let light = format!("{PREVIEW_LIGHT_YAW}/{PREVIEW_LIGHT_PITCH}");
        assert_eq!(
            cache_fingerprint(PREVIEW, picked("Quartz")),
            format!(
                "preview;tracer={};size=160;spp=256;bounces=12;lighting={};light={light};\
                 material=Quartz",
                indicatrix::BUILD_ID,
                PREVIEW_LIGHTING_PRESET.label()
            )
        );
        assert_eq!(
            cache_fingerprint(CacheKind::TiltCurves, picked("Quartz")),
            format!(
                "tilt;tracer={};lighting={};light={light};material=Quartz",
                indicatrix::BUILD_ID,
                BATCH_TILT_LIGHTING_PRESET.label()
            )
        );
        assert!(
            cache_fingerprint(PREVIEW, FingerprintMaterial::picked(None))
                .ends_with(";material=none"),
            "no material picked yet"
        );
        // A row from before the size and source columns reads as exactly that.
        let legacy = PreviewMaterialState {
            name: Some("Quartz".to_string()),
            girdle_mm: None,
            source: PreviewMaterialSource::RefractiveIndex,
        };
        assert_eq!(
            cache_fingerprint(PREVIEW, FingerprintMaterial::from_state(&legacy)),
            cache_fingerprint(PREVIEW, picked("Quartz"))
        );
        // The solid stand-in names no material at all.
        assert_eq!(
            cache_fingerprint(CacheKind::SolidDraft { size: 160 }, picked("Quartz")),
            "solid-draft;size=160"
        );
    }

    /// A design whose file names a material or a girdle diameter appends them after the
    /// material: the size with two decimals, and the source.
    #[test]
    fn a_design_files_size_and_source_are_appended_to_the_material() {
        assert!(
            cache_fingerprint(PREVIEW, from_file("Sapphire", 9.0))
                .ends_with(";material=Sapphire;size=9.00;source=design")
        );
        assert!(
            cache_fingerprint(CacheKind::TiltCurves, from_file("Sapphire", 10.874))
                .ends_with(";material=Sapphire;size=10.87;source=design")
        );
        // A file that names a material but no size.
        assert!(
            cache_fingerprint(PREVIEW, from_file("Sapphire", 0.0))
                .ends_with(";material=Sapphire;source=design")
        );
        // A size without the source cannot happen, but is still told apart.
        let sized = FingerprintMaterial {
            name: Some("Sapphire"),
            girdle_mm: 9.0,
            from_design_file: false,
        };
        assert!(cache_fingerprint(PREVIEW, sized).ends_with(";material=Sapphire;size=9.00"));
        // Different sizes differ, a difference below the printed precision does not.
        assert_ne!(
            cache_fingerprint(PREVIEW, from_file("Sapphire", 9.0)),
            cache_fingerprint(PREVIEW, from_file("Sapphire", 9.5))
        );
        assert_eq!(
            cache_fingerprint(PREVIEW, from_file("Sapphire", 9.0)),
            cache_fingerprint(PREVIEW, from_file("Sapphire", 9.001))
        );
        // Not a size: negative, not finite.
        for unset in [-1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                cache_fingerprint(PREVIEW, from_file("Sapphire", unset)),
                cache_fingerprint(PREVIEW, from_file("Sapphire", 0.0)),
                "{unset}"
            );
        }
    }

    /// The material a scan builds from a persisted row is the one the writer built from
    /// the resolved design.
    #[test]
    fn the_material_of_a_persisted_state_is_the_one_the_writer_used() {
        let state = PreviewMaterialState {
            name: Some("Sapphire".to_string()),
            girdle_mm: Some(9.0),
            source: PreviewMaterialSource::DesignFile,
        };
        assert_eq!(
            FingerprintMaterial::from_state(&state),
            from_file("Sapphire", 9.0)
        );
        assert_eq!(
            FingerprintMaterial::from_state(&PreviewMaterialState::default()),
            FingerprintMaterial::picked(None)
        );
    }

    /// The tilt fingerprint names the scoring lighting preset by its stable label, so
    /// curves stored under another preset (the ring lights before 2026-10-07) are stale.
    #[test]
    fn the_tilt_fingerprint_names_the_scoring_lighting_preset() {
        let fingerprint = cache_fingerprint(CacheKind::TiltCurves, picked("Diamond"));
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
    /// settings must not look like a reason to recompute every design's curves. (The
    /// `size=` of a design file's stone size is not one of them: it comes after the
    /// material, and an unsized material has none.)
    #[test]
    fn the_tilt_fingerprint_names_no_preview_setting() {
        let fingerprint = cache_fingerprint(CacheKind::TiltCurves, picked("Diamond"));
        for setting in ["size=", "spp=", "bounces="] {
            assert!(!fingerprint.contains(setting), "{fingerprint}");
        }
    }
}
