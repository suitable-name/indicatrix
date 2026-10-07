//! The megakernel's per-dispatch uniform: [`GpuTransportParams`] plus its
//! [`studio_model`]/[`transport_env_mode`] discriminants.

use core::mem::offset_of;

use crate::optics::raytracer::{DEFAULT_HEAD_SHADOW_COSINES, TentParams};

/// Per-dispatch kernel parameters for `shaders/spectral_transport.wgsl`'s
/// `transport_main` entry point.
///
/// `env_mode` selects which environment model to sample (`0` = the direction-independent
/// "uniform furnace" grey environment at `l0`, `1` = the analytic studio rig at the
/// given color temperature/exposure/rig-pose). `sample_offset` + `camera.num_samples`
/// select the sample range this dispatch traces, mirroring
/// `apps/indicatrix-worker/src/render_core.rs`'s `first_sample`/`samples` convention --
/// this lets `estimator_check`'s statistical comparison give the CPU and GPU DISJOINT
/// sample ranges, as production would. `white_balance` is the precomputed von-Kries
/// white balance (`compute_illuminant_white_balance`, already ULP-verified by
/// `environment_check`). This is a **Bradford LMS-space** diagonal scale, not an
/// XYZ-space one -- the megakernel's `apply_von_kries_white_balance` transforms to
/// Bradford LMS, applies this scale, and transforms back, mirroring the CPU side
/// exactly.
///
/// # Layout
///
/// This struct is 112 bytes. The `offset_of!`/`size_of!` asserts right below this
/// `impl` block are what actually pin the layout; this paragraph is descriptive, kept in
/// sync with them by hand.
///
/// The ten leading scalars pack into 40 bytes; `pixel_offset`/`write_debug_buffers` bring
/// that to 48 (a multiple of 16), so `white_balance` (`vec3<f32>`) needs no padding
/// before it, landing at offset 48 and ending at 60. `studio_use_d65` (offset 60) fills
/// the remaining 4 bytes of that 16-byte block, bringing the running total to 64 --
/// exactly what the struct WOULD be without the two fields below. `studio_model` (64)
/// and `backdrop` (68) add one more 16-byte block; `surface_glare` (72) fills the next
/// slot and `head_shadow_outer_cos` (76) completes that block (80 bytes). One more
/// 16-byte row follows: `head_shadow_inner_cos` (80), `tent_flat` (84) and two pads (88, 92,
/// genuine padding WGSL echoes but never reads). A last 16-byte row (96..112) holds the light-tent
/// parameters `tent_walls`, `tent_cards`, `tent_spark`, `tent_ground`, for 112 bytes total.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuTransportParams {
    /// Number of pixels in the dispatch.
    pub num_pixels: u32,
    /// Maximum number of internal bounces per path.
    pub max_bounces: u32,
    /// Sample offset.
    pub sample_offset: u32,
    /// Env mode.
    pub env_mode: u32,
    /// L0.
    pub l0: f32,
    /// Studio temp k.
    pub studio_temp_k: f32,
    /// Studio spot mult.
    pub studio_spot_mult: f32,
    /// Studio exposure.
    pub studio_exposure: f32,
    /// Studio light yaw.
    pub studio_light_yaw: f32,
    /// Studio light pitch.
    pub studio_light_pitch: f32,
    /// Index of the first pixel this dispatch covers, added to the shader's own
    /// `idx / num_samples` to recover a GLOBAL pixel index for camera-ray generation
    /// while output slots stay dispatch-local. Zero for a dispatch covering a whole
    /// frame (every self-test); exists for `renderer::gpu::frame`, which splits a frame
    /// too large for its memory budget into chunks.
    pub pixel_offset: u32,
    /// Whether `transport_main` should write its three per-channel debug output buffers
    /// (`out_radiance`/`out_lambdas`/`out_path_pdf`) this dispatch. Nonzero (the default)
    /// reproduces every existing dispatch's behaviour. Zero (see
    /// [`Self::with_debug_buffers_disabled`]) skips those writes -- only `out_xyz` is
    /// written -- so a production dispatch does 9x less write traffic and its chunk
    /// budget holds 9x more samples per dispatch.
    pub write_debug_buffers: u32,
    /// White balance.
    pub white_balance: [f32; 3],
    /// Whether `sample_studio_environment_with_rig` should sample the tabulated CIE D65
    /// measured spectrum (nonzero) instead of `blackbody_spectrum` at `studio_temp_k`
    /// (zero, default) -- mirrors that function's own
    /// `preset.uses_d65()` branch. Meaningless when
    /// `env_mode == transport_env_mode::UNIFORM_FURNACE`.
    pub studio_use_d65: u32,
    /// Which environment model to sample -- see [`studio_model`].
    pub studio_model: u32,
    /// Radiance of the backdrop card a camera ray sees where it misses the stone
    /// (`0.0`: none) -- mirrors `EnvironmentSource::Studio::backdrop`.
    pub backdrop: f32,
    /// Scale of the stone's first-surface specular reflection, `0.0..=1.0` (`1.0`:
    /// unchanged) -- mirrors `EnvironmentSource::Studio::surface_glare`.
    pub surface_glare: f32,
    /// Outer cosine of the observer head-shadow cone (the dot product at which the shadow
    /// starts to fade in): mirrors
    /// `head_shadow_cosines(EnvironmentSource::Studio::head_shadow_deg)[0]`. Default = the
    /// 16 degree cone's literal.
    pub head_shadow_outer_cos: f32,
    /// Inner cosine of the head-shadow cone: `head_shadow_cosines(..)[1]`.
    pub head_shadow_inner_cos: f32,
    /// Light-tent wall flattening (`TentParams::flat`; `0.0` = the tent's gradient). Lives in
    /// the first former pad slot (offset 84), so the struct size is unchanged.
    pub tent_flat: f32,
    _pad_head_shadow1: u32,
    _pad_head_shadow2: u32,
    /// Light-tent wall scale (`TentParams::walls`; `1.0` = the tent). Only the
    /// `LIGHT_TENT` model reads the four `tent_*` fields.
    pub tent_walls: f32,
    /// Light-tent card strength (`TentParams::cards`; `1.0` = the tent, `0.0` = no cards).
    pub tent_cards: f32,
    /// Light-tent spark flag (`TentParams::spark`; `1.0` = on, `0.0` = off).
    pub tent_spark: f32,
    /// Light-tent ground radiance (`TentParams::ground`; `0.02` = the tent).
    pub tent_ground: f32,
}

/// `studio_model` discriminants for [`GpuTransportParams`].
///
/// Mirrors [`crate::optics::raytracer::LightingModel`]'s `gpu_id` mapping:
/// - 0: `Studio` (classic analytic rig)
/// - 1: `IsoHemisphere` (uniform lit upper hemisphere)
/// - 2: `LightTent` (light tent + black cards)
/// - 3: `DaylightDome` (daylight sky, no sun)
/// - 4: `Aset` (ASET-style contrast view)
/// - 5: `DaylightSun` (daylight sky + direct sun)
pub mod studio_model {
    /// Identifier for studio.
    pub const STUDIO: u32 = 0;
    /// Identifier for iso hemisphere.
    pub const ISO_HEMISPHERE: u32 = 1;
    /// Identifier for light tent.
    pub const LIGHT_TENT: u32 = 2;
    /// Identifier for daylight dome.
    pub const DAYLIGHT_DOME: u32 = 3;
    /// Identifier for the ASET-style contrast view.
    pub const ASET: u32 = 4;
    /// Identifier for daylight sky plus direct sun.
    pub const DAYLIGHT_SUN: u32 = 5;
}

/// `env_mode` discriminants for [`GpuTransportParams`]. Must match
/// `shaders/spectral_transport.wgsl`'s own `params.env_mode` branch.
pub mod transport_env_mode {
    /// Identifier for uniform furnace.
    pub const UNIFORM_FURNACE: u32 = 0;
    /// Identifier for studio rig.
    pub const STUDIO_RIG: u32 = 1;
    /// `EnvironmentSource::HdrMap`.
    ///
    /// Sampled via `hdr_env_radiance_at` against the `hdr_texels`/`hdr_env_dims`
    /// storage/uniform buffers (bindings 10/11) instead of the analytic studio rig. See
    /// `renderer::env_map_gpu`'s module doc comment.
    pub const HDR_MAP: u32 = 2;
}

impl GpuTransportParams {
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "one parameter per GpuTransportParams field, in field order, because \
                  that field order is itself the #[repr(C)] layout spectral_transport.wgsl's \
                  uniform buffer binds against; bundling them into a context struct here \
                  would just re-introduce this exact struct one level removed"
    )]
    /// Creates a new value from its components.
    pub const fn new(
        num_pixels: u32,
        max_bounces: u32,
        sample_offset: u32,
        env_mode: u32,
        l0: f32,
        studio_temp_k: f32,
        studio_spot_mult: f32,
        studio_exposure: f32,
        studio_light_yaw: f32,
        studio_light_pitch: f32,
        white_balance: [f32; 3],
    ) -> Self {
        Self {
            num_pixels,
            max_bounces,
            sample_offset,
            env_mode,
            l0,
            studio_temp_k,
            studio_spot_mult,
            studio_exposure,
            studio_light_yaw,
            studio_light_pitch,
            pixel_offset: 0,
            write_debug_buffers: 1,
            white_balance,
            studio_use_d65: 0,
            studio_model: 0,
            backdrop: 0.0,
            surface_glare: 1.0,
            head_shadow_outer_cos: DEFAULT_HEAD_SHADOW_COSINES[0],
            head_shadow_inner_cos: DEFAULT_HEAD_SHADOW_COSINES[1],
            tent_flat: TentParams::DEFAULT.flat,
            _pad_head_shadow1: 0,
            _pad_head_shadow2: 0,
            tent_walls: TentParams::DEFAULT.walls,
            tent_cards: TentParams::DEFAULT.cards,
            tent_spark: TentParams::DEFAULT.spark,
            tent_ground: TentParams::DEFAULT.ground,
        }
    }

    /// Returns a copy with the light-tent per-preset parameters (see [`TentParams`]).
    /// The default already is the light tent's own values.
    #[must_use]
    pub const fn with_tent(mut self, tent: TentParams) -> Self {
        self.tent_walls = tent.walls;
        self.tent_cards = tent.cards;
        self.tent_spark = tent.spark;
        self.tent_ground = tent.ground;
        self.tent_flat = tent.flat;
        self
    }

    /// Returns a copy with the tent parameters of `environment`'s lighting preset (the
    /// default for an HDR map, whose shader branch never reads them).
    #[must_use]
    pub const fn with_tent_of(
        self,
        environment: crate::optics::raytracer::EnvironmentSource<'_>,
    ) -> Self {
        match environment.lighting_preset() {
            Some(preset) => self.with_tent(preset.params().tent),
            None => self.with_tent(TentParams::DEFAULT),
        }
    }

    /// Returns a copy covering pixels `[pixel_offset, pixel_offset + num_pixels)` of a
    /// larger frame -- see [`Self::pixel_offset`]. A separate builder rather than an
    /// eleventh `new` parameter, so every existing caller keeps its exact constructor
    /// call.
    #[must_use]
    pub const fn with_pixel_offset(mut self, pixel_offset: u32) -> Self {
        self.pixel_offset = pixel_offset;
        self
    }

    /// Returns a copy that skips `transport_main`'s three per-channel debug output
    /// writes -- see [`Self::write_debug_buffers`]'s doc comment. Only
    /// `GpuFrameRenderer::accumulate`'s production dispatch calls this.
    #[must_use]
    pub const fn with_debug_buffers_disabled(mut self) -> Self {
        self.write_debug_buffers = 0;
        self
    }

    /// Returns a copy that samples the tabulated CIE D65 measured spectrum instead of
    /// `blackbody_spectrum` at `studio_temp_k` -- see [`Self::studio_use_d65`]'s doc
    /// comment. Only callers whose `EnvironmentSource::Studio` preset is
    /// `LightingPreset::Daylight` should pass `true` here.
    #[must_use]
    pub const fn with_studio_use_d65(mut self, use_d65: bool) -> Self {
        self.studio_use_d65 = if use_d65 { 1 } else { 0 };
        self
    }

    /// Returns a copy with the specified studio lighting model (see [`studio_model`]).
    #[must_use]
    pub const fn with_studio_model(mut self, model: u32) -> Self {
        self.studio_model = model;
        self
    }

    /// Returns a copy with the specified backdrop radiance (see [`Self::backdrop`]).
    #[must_use]
    pub const fn with_backdrop(mut self, backdrop: f32) -> Self {
        self.backdrop = backdrop;
        self
    }

    /// Returns a copy with the specified surface-glare scale (see
    /// [`Self::surface_glare`]), clamped to `0.0..=1.0`; NaN means `1.0`.
    #[must_use]
    pub const fn with_surface_glare(mut self, surface_glare: f32) -> Self {
        self.surface_glare = if surface_glare >= 1.0 || surface_glare.is_nan() {
            1.0
        } else if surface_glare > 0.0 {
            surface_glare
        } else {
            0.0
        };
        self
    }

    /// Returns a copy with the observer head-shadow cone `[outer, inner]` cosines (see
    /// [`Self::head_shadow_outer_cos`] and `optics::raytracer::head_shadow_cosines`).
    #[must_use]
    pub const fn with_head_shadow(mut self, cosines: [f32; 2]) -> Self {
        self.head_shadow_outer_cos = cosines[0];
        self.head_shadow_inner_cos = cosines[1];
        self
    }
}

const _: () = {
    assert!(offset_of!(GpuTransportParams, num_pixels) == 0);
    assert!(offset_of!(GpuTransportParams, max_bounces) == 4);
    assert!(offset_of!(GpuTransportParams, sample_offset) == 8);
    assert!(offset_of!(GpuTransportParams, env_mode) == 12);
    assert!(offset_of!(GpuTransportParams, l0) == 16);
    assert!(offset_of!(GpuTransportParams, studio_temp_k) == 20);
    assert!(offset_of!(GpuTransportParams, studio_spot_mult) == 24);
    assert!(offset_of!(GpuTransportParams, studio_exposure) == 28);
    assert!(offset_of!(GpuTransportParams, studio_light_yaw) == 32);
    assert!(offset_of!(GpuTransportParams, studio_light_pitch) == 36);
    assert!(offset_of!(GpuTransportParams, white_balance) == 48);
    assert!(offset_of!(GpuTransportParams, studio_use_d65) == 60);
    assert!(offset_of!(GpuTransportParams, studio_model) == 64);
    assert!(offset_of!(GpuTransportParams, backdrop) == 68);
    assert!(offset_of!(GpuTransportParams, surface_glare) == 72);
    assert!(offset_of!(GpuTransportParams, head_shadow_outer_cos) == 76);
    assert!(offset_of!(GpuTransportParams, head_shadow_inner_cos) == 80);
    assert!(offset_of!(GpuTransportParams, tent_flat) == 84);
    assert!(offset_of!(GpuTransportParams, _pad_head_shadow1) == 88);
    assert!(offset_of!(GpuTransportParams, _pad_head_shadow2) == 92);
    assert!(offset_of!(GpuTransportParams, tent_walls) == 96);
    assert!(offset_of!(GpuTransportParams, tent_cards) == 100);
    assert!(offset_of!(GpuTransportParams, tent_spark) == 104);
    assert!(offset_of!(GpuTransportParams, tent_ground) == 108);
    assert!(size_of::<GpuTransportParams>() == 112);
};
