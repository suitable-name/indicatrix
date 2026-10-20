//! The fingerprint stored beside each cached catalogue artefact (previews and tilt
//! curves), naming what it was computed with.

use super::{
    BATCH_TILT_LIGHTING_PRESET, PREVIEW_EXPOSURE, PREVIEW_HEAD_SHADOW_DEG, PREVIEW_LIGHT_PITCH,
    PREVIEW_LIGHT_YAW, PREVIEW_LIGHTING_PRESET, PREVIEW_SURFACE_GLARE,
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_vault::model::{
    frosting::StoredTierFrosting,
    preview::{PreviewMaterialSource, PreviewMaterialState},
};
use std::{collections::BTreeMap, fmt::Write as _, sync::LazyLock};

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
    /// Persistent deterministic thumbnails.
    RayPreview {
        /// Square render dimension.
        size: u32,
        /// Fixed anti-aliasing tap count.
        taps: u8,
        /// Whether three dispersion chains are traced.
        fire: bool,
    },
    /// Temporary deterministic drafts never satisfy a persistent preview request.
    RayDraft {
        /// Square render dimension.
        size: u32,
        /// Fixed anti-aliasing tap count.
        taps: u8,
        /// Whether three dispersion chains are traced.
        fire: bool,
    },
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
    /// [`frosting_hash`] of the design's stored frosting; `0` for a polished design, which
    /// keeps the text it had before frosting was tracked. Not part of the persisted
    /// material state: a caller adds it with [`Self::with_frosting`].
    pub frosting: u64,
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
            frosting: 0,
        }
    }

    /// The material of a persisted `diagram_previews` row (`Database::
    /// get_preview_material_state`, or the state a scan is handed). Polished: add the
    /// design's frosting with [`Self::with_frosting`].
    #[must_use]
    pub fn from_state(state: &'a PreviewMaterialState) -> Self {
        Self {
            name: state.name.as_deref(),
            girdle_mm: state.girdle_mm.unwrap_or(0.0),
            from_design_file: state.source == PreviewMaterialSource::DesignFile,
            frosting: 0,
        }
    }

    /// `self` for a design whose stored frosting hashes to `frosting` ([`frosting_hash`];
    /// `0` for none), so a frosted design never matches an unfrosted cached picture.
    #[must_use]
    pub const fn with_frosting(mut self, frosting: u64) -> Self {
        self.frosting = frosting;
        self
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
        if self.frosting != 0 {
            let _ = write!(part, ";frosting={:016x}", self.frosting);
        }
        part
    }
}

/// A hash of a design's stored frosting (`Database::diagram_frosting`): the tiers and facet
/// slots it states. `0` for no frosting (so a polished design's fingerprint is unchanged),
/// never `0` for any frosting. The rows are hashed as given; the vault stores them
/// normalised (one row per tier, ascending), so the writer and the staleness scan, which
/// both read them from the vault, agree.
#[must_use]
pub fn frosting_hash(rows: &[StoredTierFrosting]) -> u64 {
    if rows.is_empty() {
        return 0;
    }
    let mut text = String::new();
    for row in rows {
        let _ = write!(text, "{}:{:?};", row.tier, row.facets);
    }
    fnv1a(text.as_bytes()).max(1)
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
///
/// A PREVIEW fingerprint also names the scene settings that are not the lighting preset
/// (the exposure, the head shadow, the backdrop and the surface glare, `PreviewScene`). Both a preview and a tilt
/// fingerprint name, for a built-in material, a hash of its band data
/// (`material_bands_hash`; both are computed through the material's absorption) and, for a
/// frosted design, a hash of its frosting ([`FingerprintMaterial::frosting`]): changed scene
/// settings, changed built-in color data or changed frosting make the stored previews and
/// curves stale on their own, without a version bump. The tilt fingerprint does not name
/// the preview scene (the tilt sweep is scored under its own preset).
#[must_use]
pub fn cache_fingerprint(kind: CacheKind, material: FingerprintMaterial<'_>) -> String {
    let bands = material.name.and_then(built_in_bands_hash);
    fingerprint_with(kind, material, PreviewScene::current(), bands)
}

/// The preview scene settings a preview fingerprint names besides the lighting preset and
/// the light pose: what the renderer takes from `PREVIEW_EXPOSURE`,
/// `PREVIEW_HEAD_SHADOW_DEG`, the thumbnail tray's name (with the tray model's version) and
/// `PREVIEW_SURFACE_GLARE`. A struct
/// (rather than the constants read inline) so a test can change one and see the fingerprint
/// move.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PreviewScene {
    /// Exposure the preview is rendered with.
    exposure: f32,
    /// Radius of the observer head shadow in degrees.
    head_shadow_deg: f32,
    /// Name of the backdrop choice (its albedo, not a radiance: the radiance follows the
    /// preview rig and pose, which the fingerprint names on their own).
    backdrop: &'static str,
    /// Version of the tray model (`TRAY_MODEL_VERSION`): a change of how the backdrop is lit
    /// makes every stored preview stale once.
    tray_model: u32,
    /// Surface glare the preview is rendered with.
    surface_glare: f32,
}

impl PreviewScene {
    /// The scene every catalogue preview is rendered in now (the tray is the user's
    /// Thumbnail Backdrop).
    fn current() -> Self {
        Self {
            exposure: PREVIEW_EXPOSURE,
            head_shadow_deg: PREVIEW_HEAD_SHADOW_DEG,
            backdrop: super::thumbnail_backdrop_name(),
            tray_model: indicatrix::optics::raytracer::TRAY_MODEL_VERSION,
            surface_glare: PREVIEW_SURFACE_GLARE,
        }
    }
}

/// 64-bit FNV-1a: stable across runs, platforms and releases of the standard library
/// (unlike `DefaultHasher`), which a stored comparison needs.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// A hash of everything that decides the color a material absorbs: its band data (every
/// ray set, band shapes included), its unit and its colour zones. The name, the
/// refractive index and the stone-size scale are NOT part of it (the size is in the
/// fingerprint already), so it moves exactly when the color data does.
///
/// Hashes the `Debug` text of the data: every `f32` prints its shortest round-trip form,
/// so the text is deterministic and covers every field, including ones added later.
#[must_use]
fn material_bands_hash(material: &GemMaterial) -> u64 {
    let text = format!(
        "{:?}|{:?}|{:?}",
        material.absorption, material.absorption_unit, material.zoning
    );
    fnv1a(text.as_bytes())
}

/// [`material_bands_hash`] of the built-in material called `name` (case-insensitive, an
/// exact name, not `GemMaterial::by_name`'s containment fallback), `None` for a name that
/// is not a built-in (a custom material).
///
/// Looked up by name because the staleness scans rebuild the fingerprint from the
/// persisted row, which holds only the name: the hash is of the built-in's data as this
/// build knows it, which is what moves when a release changes the table. A design file's
/// body-color override and an edited custom material are not seen here.
fn built_in_bands_hash(name: &str) -> Option<u64> {
    static TABLE: LazyLock<BTreeMap<String, u64>> = LazyLock::new(|| {
        GemMaterial::built_in()
            .iter()
            .map(|material| {
                (
                    material.name.to_ascii_lowercase(),
                    material_bands_hash(material),
                )
            })
            .collect()
    });
    TABLE.get(&name.to_ascii_lowercase()).copied()
}

/// [`cache_fingerprint`] with the scene settings and the built-in band hash given, so a
/// test can vary them.
fn fingerprint_with(
    kind: CacheKind,
    material: FingerprintMaterial<'_>,
    scene: PreviewScene,
    bands: Option<u64>,
) -> String {
    let tracer = indicatrix::BUILD_ID;
    let material = material.fingerprint_part();
    let light = format!("{PREVIEW_LIGHT_YAW}/{PREVIEW_LIGHT_PITCH}");
    match kind {
        CacheKind::Preview {
            size,
            spp,
            max_bounces,
        } => {
            let PreviewScene {
                exposure,
                head_shadow_deg,
                backdrop,
                tray_model,
                surface_glare,
            } = scene;
            let bands = bands.map_or_else(String::new, |hash| format!("bands={hash:016x};"));
            format!(
                "preview;tracer={tracer};size={size};spp={spp};bounces={max_bounces};\
                 lighting={};light={light};exposure={exposure};shadow={head_shadow_deg};\
                 backdrop={backdrop};tray={tray_model};glare={surface_glare};{bands}{material}",
                PREVIEW_LIGHTING_PRESET.label()
            )
        }
        CacheKind::RayPreview { size, taps, fire } | CacheKind::RayDraft { size, taps, fire } => {
            let prefix = if matches!(kind, CacheKind::RayPreview { .. }) {
                "ray-preview"
            } else {
                "ray-draft"
            };
            let version = indicatrix::renderer::whitted::WHITTED_VERSION;
            let PreviewScene {
                exposure,
                head_shadow_deg,
                backdrop,
                tray_model,
                surface_glare,
            } = scene;
            let bands = bands.map_or_else(String::new, |hash| format!("bands={hash:016x};"));
            format!(
                "{prefix};v={version};size={size};taps={taps};fire={fire};lighting={};light={light};exposure={exposure};shadow={head_shadow_deg};backdrop={backdrop};tray={tray_model};glare={surface_glare};{bands}{material}",
                PREVIEW_LIGHTING_PRESET.label()
            )
        }
        CacheKind::SolidDraft { size } => format!("solid-draft;size={size}"),
        CacheKind::TiltCurves => {
            let bands = bands.map_or_else(String::new, |hash| format!("bands={hash:016x};"));
            format!(
                "tilt;tracer={tracer};lighting={};light={light};{bands}{material}",
                BATCH_TILT_LIGHTING_PRESET.label()
            )
        }
    }
}

#[cfg(test)]
#[path = "fingerprint_tests.rs"]
mod tests;
