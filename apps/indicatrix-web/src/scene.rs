//! The scene this crate renders: facet geometry parsed from an uploaded `.asc` file,
//! plus the small set of material/lighting/camera parameters `ui/app.slint` exposes.
//!
//! Deliberately not `indicatrix_net::SceneState` (shared by `apps/indicatrix-cut` and
//! `apps/indicatrix-worker`): that type is a wire-protocol contract with a remote
//! worker, and this crate has none -- see `README.md`. A local struct is simpler than
//! depending on `indicatrix-net` for one type (pulling in unused `postcard`).

use indicatrix::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};

/// The camera field of view, in degrees. Fixed rather than user-adjustable -- one less
/// control this crate's small surface needs, and a stone's proportions read most
/// naturally at a moderate FOV.
pub const FOV_DEG: f32 = 42.0;

/// The smallest render-target edge this crate will ever allocate, in physical pixels.
/// `ui/app.slint`'s render surface can transiently report `0` or a handful of pixels
/// (the first layout pass, or a hidden/zero-area tab); without a floor that would
/// zero-size [`crate::render::Accumulator`]'s buffer. `64` is arbitrary beyond "clearly
/// above zero, clearly below any real layout".
pub const MIN_RENDER_DIM: u32 = 64;

/// The largest render-target edge this crate will ever allocate, in physical pixels,
/// regardless of how large `ui/app.slint`'s render surface lays out to. Cost scales
/// linearly with pixel count, and the surface stretches to fill the viewport (well over
/// 3000 logical px wide on a maximised 4K monitor); `960` keeps the worst case
/// (921,600px) to roughly 3x the original fixed 660x480 box, a deliberate bounded
/// increase. The displayed image is unaffected: `ui/app.slint`'s `image-fit: contain`
/// upscales a smaller rendered buffer to fill the surface.
pub const MAX_RENDER_DIM: u32 = 960;

/// The largest device pixel ratio this crate applies when converting `ui/app.slint`'s
/// logical render-surface size into a physical render-target size. Applying DPR in
/// full is a quadratic cost multiplier (a 2x-DPR laptop screen quadruples pixel count;
/// some phones report 3x+ for 9x the pixels); `1.5` caps the multiplier at 2.25x while
/// staying crisper than ignoring DPR. See [`clamp_render_dims`].
pub const MAX_DEVICE_PIXEL_RATIO: f32 = 1.5;

/// Converts `ui/app.slint`'s render surface's logical size (from its
/// `render-size-changed` callback, wired by `src/app.rs`) and the window's device
/// pixel ratio into a physical render-target size clamped to this crate's range.
///
/// `scale_factor` should be `slint::Window::scale_factor()`, read fresh at the moment
/// the resize settles rather than cached, since a page can move between monitors with
/// different DPRs without any resize of its own.
///
/// # Panics
///
/// Never -- `logical_width`/`logical_height` are clamped through `f32::max(0.0)`
/// before rounding, so a negative or NaN report saturates to [`MIN_RENDER_DIM`] rather
/// than panicking on an out-of-range cast.
#[must_use]
pub fn clamp_render_dims(logical_width: f32, logical_height: f32, scale_factor: f32) -> (u32, u32) {
    let dpr = scale_factor.clamp(1.0, MAX_DEVICE_PIXEL_RATIO);
    let physical_width = (logical_width.max(0.0) * dpr).round() as u32;
    let physical_height = (logical_height.max(0.0) * dpr).round() as u32;
    (
        physical_width.clamp(MIN_RENDER_DIM, MAX_RENDER_DIM),
        physical_height.clamp(MIN_RENDER_DIM, MAX_RENDER_DIM),
    )
}

/// The bounce budget this crate uses, copied from `apps/indicatrix-cut`'s own default:
/// a gemstone's brilliance comes overwhelmingly from the first handful of internal
/// reflections, and this crate's whole render path is the WebGPU megakernel, with no
/// CPU-side per-bounce cost to protect.
pub const MAX_BOUNCES: u32 = 12;

/// The four material presets `ui/app.slint`'s `material-options` combo box lists, in
/// the same order. A closed, small set rather than the custom-material editor
/// `apps/indicatrix-cut` has -- see `README.md`.
pub fn material_for_index(index: i32) -> GemMaterial {
    match index {
        1 => GemMaterial::ruby(),
        2 => GemMaterial::sapphire(),
        3 => GemMaterial::emerald(),
        _ => GemMaterial::diamond(),
    }
}

/// The material combo box's starting selection (Diamond) -- seeds `ui/app.slint`'s
/// `material-index` property before the first render.
pub const DEFAULT_MATERIAL_INDEX: i32 = 0;

/// `ui/app.slint`'s `lighting-options` combo box, resolved through
/// [`LightingPreset::from_index`] -- this crate exposes the seven presets
/// `indicatrix` ships.
pub const fn lighting_for_index(index: i32) -> LightingPreset {
    LightingPreset::from_index(index)
}

/// A successfully parsed `.asc` schedule, turned into real facet planes -- everything
/// [`crate::render`] needs about the stone, independent of camera/material/lighting
/// (those live in [`ViewState`], since they change far more often: parsing a new file
/// is rare, dragging to orbit is not).
pub struct StoneGeometry {
    pub planes: Vec<GpuFacetPlane>,
    /// The file name as the browser reported it, shown in `ui/app.slint`'s status line
    /// so a user juggling several schedules can tell which one is loaded.
    pub file_name: String,
}

/// Parses `contents` (an uploaded `.asc` file's bytes, assumed UTF-8) into facet planes.
///
/// # Errors
///
/// A human-readable message, never a panic: a file that isn't valid UTF-8, or one
/// that's text but not a schedule [`indicatrix_formats::asc::parse_asc`] recognizes. Both are
/// ordinary user mistakes, not bugs.
pub fn parse_uploaded_asc(file_name: &str, contents: &[u8]) -> Result<StoneGeometry, String> {
    let text = std::str::from_utf8(contents).map_err(|_| {
        format!(
            "\"{file_name}\" isn't a text file -- a GemCAD .asc cutting schedule is \
             plain text. Make sure this is the .asc file itself, not an image or a \
             design-software binary export."
        )
    })?;
    let schedule = indicatrix_formats::asc::parse_asc(text).map_err(|e| {
        format!("\"{file_name}\" doesn't look like a GemCAD .asc cutting schedule: {e}")
    })?;
    let planes = StandardGemCuts::from_asc_schedule(&schedule);
    if planes.is_empty() {
        return Err(format!(
            "\"{file_name}\" parsed, but it has no facet tiers ('a' records) at all -- \
             there is no geometry to render."
        ));
    }
    Ok(StoneGeometry {
        planes,
        file_name: file_name.to_string(),
    })
}

/// Camera/material/lighting/exposure -- the parameters `ui/app.slint`'s controls edit
/// directly, snapshotted into an owned value each time [`crate::render`] starts a new
/// frame (a `.await`-ing render task needs a snapshot, not a live borrow).
#[derive(Clone)]
pub struct ViewState {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub material_index: i32,
    pub lighting_index: i32,
    pub exposure: f32,
    /// The current render-target size, in physical pixels -- already clamped by
    /// [`clamp_render_dims`]. Lives here alongside camera/material/lighting so a resize
    /// is just one more member of the set `app.rs`'s `render_loop` snapshots and
    /// [`crate::render::Accumulator::reset`] re-runs against.
    pub render_width: u32,
    pub render_height: u32,
}

impl Default for ViewState {
    /// Matches `ui/app.slint`'s own property defaults exactly (`yaw: 0.6`,
    /// `pitch: 0.35`, `distance: 4.2`, `exposure: 1.0`) -- both sides hard-code the
    /// same values since Slint property defaults aren't reachable from Rust before the
    /// window exists. Nothing enforces this agreement at compile time.
    ///
    /// `render_width`/`render_height` start at the crate's original fixed 660x480, not
    /// `0`, as a defensive fallback in case a render is requested before
    /// `render-size-changed` has reported a real size.
    fn default() -> Self {
        Self {
            yaw: 0.6,
            pitch: 0.35,
            distance: 4.2,
            material_index: DEFAULT_MATERIAL_INDEX,
            lighting_index: 0,
            exposure: 1.0,
            render_width: 660,
            render_height: 480,
        }
    }
}
