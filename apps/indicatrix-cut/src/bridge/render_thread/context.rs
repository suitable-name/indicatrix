//! The `RenderContext`/`FrameInputs` state: the shared, live-mutated render
//! configuration the GUI writes into and the render loop reads a per-frame snapshot
//! from, plus material resolution and per-frame quality derivation.

use crate::{
    bridge::frame_cache::stone_width::StoneWidthCache,
    settings::model::{LiveComputeTarget, LocalComputeTarget, LocalPreviewScale},
};
use glam::Vec3;
use indicatrix::{
    geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane},
    optics::{
        materials::{GemMaterial, OpticalCharacter},
        raytracer::LightingPreset,
    },
    renderer::env_map::{EnvMapError, EnvironmentMap},
};
use indicatrix_net::client::Accumulator;
use std::sync::{Arc, Mutex};

pub struct RenderContext {
    /// Live render resolution, set via `settings_dialog.slint`'s pill selector
    /// (`gui::mod::on_resolution_changed`). Restricted to fixed choices (640x480 ...
    /// 1920x1080) because Slint has no way to report a widget's rendered size back to
    /// Rust. Always the CONFIGURED resolution -- `local_preview_scale`/`camera_moving`
    /// shadow a reduced copy per-frame (see `local_preview::effective_dimensions`) but
    /// never mutate these, so export/remote-render sizing is unaffected by preview
    /// scaling. A change here is picked up by `update_accumulation_state`, which resets
    /// accumulation and the guide/framebuffer transfer on the next frame.
    pub width: u32,
    pub height: u32,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub light_yaw: f32,
    pub light_pitch: f32,
    pub material_name: String,
    /// A fully-resolved material that, when present, takes priority over
    /// `material_name` in [`resolve_material_with_override`] -- CAD audit items 57
    /// and 61. `material_name`'s plain by-name lookup (see [`resolve_material`])
    /// cannot represent "this design's real effective material" for a design with
    /// no `material.name` set (every `.asc`-imported or brand-new design, per
    /// `crates/indicatrix-cut-core/src/design/construct.rs`) or with an RI
    /// override typed against an unlisted material -- both currently fall back to
    /// silently tracing as Diamond. The intended writer is `gui::editor::view::
    /// refresh_design_settings`, via `gui::editor::material_lookup::
    /// resolved_gem_material` (already override-aware) -- `bridge::render_thread`
    /// sits below `gui::` and resolves materials by value on purpose, so it has no
    /// way to call that resolver itself. `None` (default) reproduces the
    /// pre-existing by-name-only resolution exactly.
    pub material_override: Option<GemMaterial>,
    pub lighting_preset: LightingPreset,
    /// What the camera sees behind the stone -- `AppSettings::backdrop`.
    pub backdrop: crate::settings::model::Backdrop,
    /// Progressive-accumulation target; the render loop stops once `accum_samples`
    /// reaches this. Samples-per-frame is derived from it, not chosen directly -- see
    /// `resolve_material_and_quality`.
    pub target_samples: u32,
    pub max_bounces: u32,
    pub exposure: f32,
    /// Inclusion/subsurface scattering amount, applied via
    /// `GemMaterial::with_scattering_amount`. `0.0` (default) is off; useful range is
    /// `0.05` (barely perceptible) to `3.0` (milky) -- see `scattering_sigma_s`.
    pub inclusion_sigma_s: f32,
    /// Crystal-axis orientation override: `Some(axis)` replaces the resolved
    /// material's `c_axis` (skipped for isotropic materials); `None` (default, "as
    /// cut") leaves it untouched. Already resolved to a `Vec3` -- `gui::c_axis::
    /// angles_to_c_axis` converts the settings dialog's tilt/azimuth sliders here.
    pub c_axis_override: Option<Vec3>,
    /// Bruted (frosted) girdle finish toggle: `true` renders the identified girdle
    /// band as `FacetFinish::Frosted` instead of `Polished` -- see
    /// `GirdleFinishCache`. `false` (default) is the all-polished path.
    pub girdle_frosted: bool,
    /// Facet edge (meet-point) rounding radius, via `GemMaterial::with_edge_rounding`.
    /// `0.0` (default) is off (sharp edges). See `edge_rounding_radius` in
    /// `crates/indicatrix/src/optics/materials.rs` for units/range.
    pub edge_rounding_radius: f32,
    /// Physical stone size: girdle width in millimetres the active design should be
    /// treated as measuring, for absorption/scattering. `0.0` (default) is off --
    /// every built-in cut already renders at `absorption_path_scale = 1.0`. When
    /// positive, `apply_material_overrides` divides this by the design's measured
    /// model-unit girdle width (`stone_metrics::measure_solid`, cached by
    /// `StoneWidthCache`) to get the scale factor, so e.g. "6.5" renders the current
    /// cut as if cut from a 6.5mm rough, without changing any facet angle.
    pub stone_width_mm: f32,
    /// The active design's facet planes, and the catalogue's custom materials.
    ///
    /// `Arc<Vec<..>>`, not a bare `Vec`: `snapshot_frame_inputs` reads both out of this
    /// struct on every render-loop iteration (~every 16ms), and `GemMaterial` carries
    /// `String`s/`Vec`s of its own, so a deep clone of either field is real,
    /// non-trivial work repeated 60x/second for data that is usually unchanged frame
    /// to frame. An `Arc` clone there is one atomic increment instead. A writer that
    /// needs to mutate in place (rather than replace wholesale) goes through
    /// `Arc::make_mut` (see `gui::optics::custom_materials`), which only pays for an
    /// actual deep copy on the rare frame where a render-loop snapshot is still
    /// outstanding.
    pub active_planes: Arc<Vec<GpuFacetPlane>>,
    /// The active design's gear tooth count and reference angle -- meant to be kept
    /// in sync with `active_planes` by every one of its writers (Grep `active_planes
    /// =` for the current list: `gui::editor::view::refresh_viewport`, `gui::editor::
    /// auto_solve`'s background-solve resubmit, `gui::library::detail::
    /// apply_reconstructed_planes`, `gui::library::local::organize`'s delete-reset).
    ///
    /// Exists so `gui::render::camera_lighting::resubmit_at_current_pose`'s Diagram-
    /// mode `Reproject` requests (a camera-drag redraw, which carries no `Design` of
    /// its own) can hand the solid-preview worker the CURRENTLY loaded design's gear
    /// info, rather than whatever a previous editor `Replan` happened to leave behind
    /// in the worker's own `solid_preview::preview_state::DiagramMemory` -- which, for
    /// a design loaded via the library (never replanned through the editor), would be
    /// a stale/unrelated design's gear info, not this one's.
    ///
    /// `None` until a writer sets it -- a `Reproject` request built from `None` leaves
    /// the worker's own last-known gear info untouched (see
    /// `solid_preview::preview_state::SolidPreviewState::request_redraw`'s doc
    /// comment), which is a fine default for callers that have no design of their own
    /// to report (the built-in placeholder cut, a deleted-selection reset).
    pub design_gear: Option<(u32, f32)>,
    /// Which of `active_planes`'s four writers most recently claimed the slot --
    /// CAD audit item 58. `active_planes`/`design_gear` have no ownership check
    /// today: an editor solve, the editor's own background auto-solve, a catalogue
    /// row selection, and a catalogue-row delete-reset all write them
    /// unconditionally, so browsing the catalogue while editing a design silently
    /// swaps out the design every downstream reader (the tracer, the metrics HUD,
    /// the tilt sweep/hover preview, export, remote render) describes.
    ///
    /// This field only RECORDS the claim -- see [`Self::claim_active_planes`], the
    /// single point meant to set `active_planes`/`design_gear`/`planes_owner`
    /// together. It does not by itself refuse or arbitrate anything: each of the
    /// four writers (`gui::editor::view::refresh_viewport`, `gui::editor::
    /// auto_solve`'s background-solve resubmit, `gui::library::detail::
    /// apply_reconstructed_planes`, `gui::library::local::organize`'s delete-reset)
    /// still needs to switch to calling [`Self::claim_active_planes`] instead of
    /// assigning the three fields directly, and `apply_reconstructed_planes`/the
    /// delete-reset still need to consult [`Self::planes_owner`] before deciding
    /// whether to overwrite an `Editor` owner's in-progress work (see that
    /// method's own doc comment for the exact check).
    pub planes_owner: PlanesOwner,
    /// Why the design currently on the bench cannot be traced honestly, as a
    /// cutter-facing sentence -- `None` when it can (CAD audit item 57).
    ///
    /// Set when a design names no material AND its own refractive index matches no
    /// built-in preset within tolerance. The old behaviour was to silently trace it
    /// as Diamond, so a quartz design's windowing, extinction and tilt curve were a
    /// diamond simulation while MARGIN and the critical angle beside them used the
    /// real RI -- two numbers on screen contradicting each other with no hint why.
    /// Refusing and saying so is the owner's chosen behaviour over substituting
    /// something plausible.
    pub material_unresolved: Option<String>,
    pub custom_materials: Arc<Vec<GemMaterial>>,
    /// Shutdown signal for the render thread. Setting this `false` ends the loop
    /// *permanently* -- never reuse this as a pause mechanism; see `paused`.
    pub running: bool,
    pub dirty: bool,
    /// User-initiated pause/stop control, independent of `tab_visible`: both suspend
    /// rendering when off, but switching tabs must never clear an explicit pause, and
    /// pausing must never look like a tab-visibility change.
    pub paused: bool,
    /// Automatic suspend when the rendered image isn't visible anywhere: combines the
    /// UI's `active_tab`, the Live Render/Edit sub-tab, and whether Live Render has
    /// been popped into its own OS window -- see
    /// `gui::detached_render::setup_live_render_visibility_callbacks`, the single place
    /// that computes this flag. Not user-facing on its own.
    pub tab_visible: bool,
    /// Whether the À-Trous denoiser is applied to the tone-mapped output -- see
    /// `AppSettings::denoise_enabled`. `true` by default. Independent of
    /// `remote_active`: this is about WHETHER to denoise, not which backend produced
    /// the samples.
    pub denoise_enabled: bool,
    /// Set by the remote-rendering orchestrator while a remote worker owns the
    /// displayed image; suspends local tracing like `paused`/`tab_visible` so local
    /// samples never accumulate into a buffer a remote frame is about to replace.
    /// Distinct from `paused`: driven by the handoff state machine, not the user, and
    /// must never be observable as a user-visible pause.
    ///
    /// Stays `true` past a successful completion (`RemoteUpdate::Done`), not just
    /// through `Settling`/`RemoteRendering`: a finished remote render needs no local
    /// improvement, and resuming immediately would let local race back in and
    /// progressively overwrite it. Cleared only by `HandoffAction::DiscardRemotePartial`
    /// (a failed/cancelled attempt) or `resolve_remote_ownership` (the single choke
    /// point releasing a *completed* render's ownership on the next real scene
    /// invalidation).
    ///
    /// For [`LiveComputeTarget::Both`] the render loop no longer suspends tracing while
    /// this is `true` (see `SuspensionFlags`): local keeps tracing past whatever
    /// `remote_reserved_samples` reserves, and this field instead gates whether the
    /// display cycle folds [`remote_accumulator`](Self::remote_accumulator) into the
    /// shown image (see `should_combine_remote`).
    pub remote_active: bool,
    /// The remote accumulator backing the current settle's dispatched render, when
    /// `live_compute_target == Both`. Set together with `remote_active`/`dirty`/
    /// [`remote_reserved_samples`](Self::remote_reserved_samples) in one locked
    /// mutation by `start_remote_render`, cleared by the same discard paths that clear
    /// `remote_active`. Not cleared on `RemoteUpdate::Done` -- its contribution keeps
    /// being folded into the combined image for the rest of the settle.
    ///
    /// `Arc<Mutex<..>>`: the same accumulator instance the remote worker thread sums
    /// `FRAME` deltas into. The render thread only calls `buffer()`/`samples_done()` on
    /// it -- never `last_preview`, since a `PREVIEW` snapshot must never reach a
    /// full-resolution accumulator.
    pub remote_accumulator: Option<Arc<Mutex<Accumulator>>>,
    /// How many absolute sample indices `[0, remote_reserved_samples)` are reserved for
    /// the remote render dispatched this settle -- `0` when nothing is reserved. Local's
    /// per-frame `sample_offset` is this value plus samples traced so far this epoch, so
    /// local's indices always start where remote's range ends (the same disjointness
    /// arithmetic `export_thread::run_export` uses, here specialised to a fixed
    /// `[0, remote_render_samples)` request rather than a calibrated split).
    pub remote_reserved_samples: u32,
    /// Set by `gui::render_export` for the duration of a high-resolution export;
    /// suspends local tracing like `paused`/`tab_visible`/`remote_active` so the
    /// viewport stops burning CPU (and contending for `GpuBackend`'s shared `Mutex`) on
    /// a picture nobody is watching. Must never be observable as a user-visible pause --
    /// flipping `paused` instead would corrupt its restore. Cleared on every exit path
    /// (success/error/cancel) by the same `on_done` callback that resets
    /// `is_exporting`, so a failed export can't freeze the viewport.
    pub export_active: bool,
    /// The one-shot remote render's total sample budget, read live by
    /// `start_remote_render` at dispatch time (not cached). `512` by default, matching
    /// the old hardcoded `REMOTE_RENDER_SAMPLES`.
    pub remote_render_samples: u32,
    /// Live rendering's Local/Remote/Local+Remote choice -- see
    /// `LiveComputeTarget`. Read fresh by `orchestrator::poll_tick` at every settle
    /// (so a change applies on the NEXT settle) and every frame by the render loop.
    pub live_compute_target: LiveComputeTarget,
    /// Local live-rendering CPU/CPU+GPU/GPU choice -- see `LocalComputeTarget`. Read
    /// fresh every frame by `accumulate_frame_samples`, so a settings change applies
    /// immediately with no restart. `CpuGpu` (default) is the pre-existing hybrid
    /// behaviour -- see that function's doc comment for how.
    pub local_compute_target: LocalComputeTarget,
    /// Local preview-then-settle rendering: resolution reduction while the camera is
    /// moving. `Off` (default) makes `local_preview::effective_dimensions` return
    /// `width`/`height` unchanged regardless of `camera_moving`.
    pub local_preview_scale: LocalPreviewScale,
    /// Whether the camera is CURRENTLY moving -- mirrors
    /// `HandoffState::Previewing`, written once per poll tick from the same
    /// `HandoffMachine` that drives the remote handoff, so there's one definition of
    /// "settled" app-wide. Meant to be mutually exclusive with `remote_active`;
    /// `resolve_remote_ownership` releases `remote_active` on the same camera-drag
    /// `ctx.dirty = true` write that will make this go `true` on the next poll tick.
    /// The render loop still ANDs this with `!remote_active` before applying
    /// `local_preview_scale` as a belt-and-suspenders guard.
    pub camera_moving: bool,
    /// HDR environment maps: `Some(map)` replaces the analytic studio rig with a
    /// loaded Radiance `.hdr` panorama as the render loop's `EnvironmentSource` -- see
    /// `indicatrix::renderer::env_map`. `None` (default) is the studio-rig-only path.
    ///
    /// `Arc`, not a bare `EnvironmentMap`: a decoded panorama can be tens of megabytes,
    /// and `snapshot_frame_inputs` clones every field under the lock every frame -- an
    /// `Arc` clone is one atomic increment vs. re-copying the buffer 60x/second.
    /// `gui::mod`'s load/clear callbacks are the only writers.
    pub env_map: Option<Arc<EnvironmentMap>>,
}

/// Tags which subsystem last claimed `RenderContext::active_planes`/`design_gear`
/// -- CAD audit item 58. See [`RenderContext::claim_active_planes`] for the
/// intended write path and [`RenderContext::planes_owner`] for why this exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlanesOwner {
    /// Nobody has claimed the slot since startup -- still the built-in
    /// placeholder cut [`RenderContext::default`] seeds `active_planes` with.
    #[default]
    Builtin,
    /// The editor's currently loaded/edited design, tagged with `EditorState`'s
    /// own edit generation counter so a superseded write (e.g. a background
    /// auto-solve that finished after a newer edit started) can be told apart
    /// from the current one.
    Editor {
        /// `EditorState`'s generation counter at claim time.
        generation: u64,
    },
    /// A catalogue entry, tagged by its row id so a delete-reset (item 145) can
    /// check whether the row it just deleted is the one that owns the slot before
    /// resetting it.
    Catalogue {
        /// The catalogue row's database id.
        entry_id: i64,
    },
}

impl RenderContext {
    /// Whether a write tagged `new_owner` may overwrite the slot's CURRENT owner
    /// -- CAD audit item 58's arbitration rule, factored out so every writer
    /// applies the same policy instead of five copies of an `if` chain.
    ///
    /// An `Editor` owner always wins over anything else: the cutter is actively
    /// working on a design, and a catalogue glance/delete must never silently
    /// replace what they are editing. Two `Editor` claims arbitrate by
    /// generation, so a background auto-solve that was already stale when it
    /// finished can't clobber a newer edit's planes. Anything else (a fresh
    /// catalogue selection over `Builtin`/another `Catalogue` row, or the very
    /// first claim from `Builtin`) is allowed.
    #[must_use]
    pub const fn may_claim_active_planes(&self, new_owner: PlanesOwner) -> bool {
        match (self.planes_owner, new_owner) {
            (
                PlanesOwner::Editor {
                    generation: current,
                },
                PlanesOwner::Editor { generation: new },
            ) => new >= current,
            (PlanesOwner::Editor { .. }, _) => false,
            _ => true,
        }
    }

    /// The single intended write path for `active_planes`/`design_gear`/
    /// `planes_owner` together -- CAD audit item 58. Returns `false` (and leaves
    /// every field untouched) when [`Self::may_claim_active_planes`] refuses the
    /// claim, so a caller can decide whether to surface that as a toast/prompt.
    ///
    /// Does NOT set `dirty` -- callers already do that themselves alongside
    /// whatever else a plane-set change requires (a `Reproject`/`Replan` request,
    /// a tilt-sweep re-request per item 147, etc.), and folding it in here would
    /// make a refused claim's caller have to remember to skip that too.
    pub fn claim_active_planes(
        &mut self,
        planes: Arc<Vec<GpuFacetPlane>>,
        design_gear: Option<(u32, f32)>,
        owner: PlanesOwner,
    ) -> bool {
        if !self.may_claim_active_planes(owner) {
            return false;
        }
        self.active_planes = planes;
        self.design_gear = design_gear;
        self.planes_owner = owner;
        true
    }

    /// CAD audit item 59: whether the currently traced/rasterized `active_planes`
    /// describe an OLDER edit than `current_generation` -- the "render is stale"
    /// signal that finding's STATUS note says is still missing (no
    /// `planes_generation`/`trace_stale` property exists anywhere in the app).
    /// [`PlanesOwner::Editor`] already carries exactly the generation
    /// `active_planes` was last claimed at (CAD audit item 58), so this is a
    /// direct comparison against it rather than a new field -- `false` whenever
    /// the slot is not even owned by the editor (`Builtin`/`Catalogue`), since
    /// "stale relative to an edit" only means something while the editor owns the
    /// slot at all.
    ///
    /// This alone does not close item 59: a caller still needs to call it with
    /// `EditorState::generation`'s live value (not this lane's file to read from)
    /// and push the result into a new Slint property with an amber overlay in the
    /// Path-traced/Both viewport (`ui/**`, also not this lane's file) -- see this
    /// fix's own handoff notes for the exact wiring.
    #[must_use]
    pub const fn traced_planes_are_stale(&self, current_generation: u64) -> bool {
        matches!(
            self.planes_owner,
            PlanesOwner::Editor { generation } if generation != current_generation
        )
    }
}

impl Default for RenderContext {
    fn default() -> Self {
        Self {
            width: 800,
            height: 600,
            yaw: 0.60,   // 35 degrees azimuthal
            pitch: 0.45, // 26 degrees elevation (showing crown, table, and pavilion sparkle in 3D)
            distance: 2.4,
            light_yaw: 0.85,   // ~48 degrees azimuth
            light_pitch: 0.95, // ~54 degrees elevation
            material_name: "Diamond".to_string(),
            material_override: None,
            lighting_preset: LightingPreset::RingLights,
            backdrop: crate::settings::model::Backdrop::default(),
            target_samples: 256,
            max_bounces: 12,
            exposure: 1.0,
            inclusion_sigma_s: 0.0,
            c_axis_override: None,
            girdle_frosted: false,
            edge_rounding_radius: 0.0,
            stone_width_mm: 0.0,
            active_planes: Arc::new(StandardGemCuts::standard_round_brilliant()),
            design_gear: None,
            planes_owner: PlanesOwner::Builtin,
            material_unresolved: None,
            custom_materials: Arc::new(Vec::new()),
            running: true,
            dirty: true,
            paused: false,
            tab_visible: true,
            denoise_enabled: true,
            remote_active: false,
            remote_accumulator: None,
            remote_reserved_samples: 0,
            export_active: false,
            remote_render_samples: 512,
            live_compute_target: LiveComputeTarget::Both,
            local_compute_target: LocalComputeTarget::CpuGpu,
            local_preview_scale: LocalPreviewScale::Off,
            camera_moving: false,
            env_map: None,
        }
    }
}

/// Decodes a Radiance `.hdr` file at `path` into an [`EnvironmentMap`], wrapped in the
/// `Arc` `RenderContext::env_map` stores. `gui::mod`'s load callback shows an `Err` via
/// the toast mechanism and never assigns into `env_map`, so a bad file leaves the
/// previously active environment untouched.
///
/// # Errors
///
/// Returns `Err` with a human-readable message if `path` cannot be read or does not
/// decode as a valid Radiance HDR image.
pub fn load_env_map(path: &str) -> Result<Arc<EnvironmentMap>, String> {
    EnvironmentMap::from_hdr_file(path)
        .map(Arc::new)
        .map_err(|e: EnvMapError| e.to_string())
}

/// One frame's worth of inputs read out of `RenderContext` under its lock, copied out
/// so the mutex guard can be dropped immediately.
pub(super) struct FrameInputs {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) yaw: f32,
    pub(super) pitch: f32,
    pub(super) distance: f32,
    pub(super) light_yaw: f32,
    pub(super) light_pitch: f32,
    pub(super) material_name: String,
    pub(super) material_override: Option<GemMaterial>,
    /// See [`RenderContext::material_unresolved`] -- CAD audit item 57.
    pub(super) material_unresolved: Option<String>,
    pub(super) lighting_preset: LightingPreset,
    pub(super) backdrop: crate::settings::model::Backdrop,
    pub(super) target_samples: u32,
    pub(super) max_bounces: u32,
    pub(super) exposure: f32,
    pub(super) inclusion_sigma_s: f32,
    pub(super) c_axis_override: Option<Vec3>,
    pub(super) girdle_frosted: bool,
    pub(super) edge_rounding_radius: f32,
    pub(super) stone_width_mm: f32,
    pub(super) active_planes: Arc<Vec<GpuFacetPlane>>,
    pub(super) custom_materials: Arc<Vec<GemMaterial>>,
    pub(super) running: bool,
    pub(super) dirty: bool,
    pub(super) paused: bool,
    pub(super) tab_visible: bool,
    pub(super) denoise_enabled: bool,
    pub(super) remote_active: bool,
    pub(super) remote_accumulator: Option<Arc<Mutex<Accumulator>>>,
    pub(super) remote_reserved_samples: u32,
    pub(super) export_active: bool,
    pub(super) live_compute_target: LiveComputeTarget,
    pub(super) local_compute_target: LocalComputeTarget,
    pub(super) local_preview_scale: LocalPreviewScale,
    pub(super) camera_moving: bool,
    pub(super) env_map: Option<Arc<EnvironmentMap>>,
}

/// Snapshots every field the render loop needs for one frame out of `RenderContext`,
/// clearing `dirty` in the same locked section so a `dirty` set by a callback between
/// the read and the clear is never lost.
pub(super) fn snapshot_frame_inputs(ctx: &Arc<Mutex<RenderContext>>) -> FrameInputs {
    // Recovers from a poisoned lock rather than panicking, matching every other
    // `RenderContext` lock in this crate. This runs once per frame on the render
    // thread, so panicking here would permanently kill rendering while the UI thread
    // (recovering the same way) keeps servicing the window -- indistinguishable from a
    // hang, with no console in a release build to show why. Every field is a plain
    // value written under this same lock, so the worst a poisoning writer leaves
    // behind is a stale-but-valid frame, overwritten next tick anyway.
    let mut ctx = ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dirty = ctx.dirty;
    ctx.dirty = false;
    FrameInputs {
        width: ctx.width,
        height: ctx.height,
        yaw: ctx.yaw,
        pitch: ctx.pitch,
        distance: ctx.distance,
        light_yaw: ctx.light_yaw,
        light_pitch: ctx.light_pitch,
        material_name: ctx.material_name.clone(),
        material_override: ctx.material_override.clone(),
        material_unresolved: ctx.material_unresolved.clone(),
        lighting_preset: ctx.lighting_preset,
        backdrop: ctx.backdrop,
        target_samples: ctx.target_samples,
        max_bounces: ctx.max_bounces,
        exposure: ctx.exposure,
        inclusion_sigma_s: ctx.inclusion_sigma_s,
        c_axis_override: ctx.c_axis_override,
        girdle_frosted: ctx.girdle_frosted,
        edge_rounding_radius: ctx.edge_rounding_radius,
        stone_width_mm: ctx.stone_width_mm,
        // `Arc::clone`, not a deep copy -- see `RenderContext::active_planes`'s doc
        // comment.
        active_planes: Arc::clone(&ctx.active_planes),
        // `Arc::clone`, not a deep copy -- see `RenderContext::custom_materials`'s doc
        // comment.
        custom_materials: Arc::clone(&ctx.custom_materials),
        running: ctx.running,
        dirty,
        paused: ctx.paused,
        tab_visible: ctx.tab_visible,
        denoise_enabled: ctx.denoise_enabled,
        remote_active: ctx.remote_active,
        // `Arc::clone`, not a deep copy -- see `remote_accumulator`'s doc comment.
        remote_accumulator: ctx.remote_accumulator.clone(),
        remote_reserved_samples: ctx.remote_reserved_samples,
        export_active: ctx.export_active,
        live_compute_target: ctx.live_compute_target,
        local_compute_target: ctx.local_compute_target,
        local_preview_scale: ctx.local_preview_scale,
        camera_moving: ctx.camera_moving,
        // `Arc::clone`, not a deep copy of the decoded panorama -- see `env_map`'s doc comment.
        env_map: ctx.env_map.clone(),
    }
}

/// Resolves the current gem material by name: custom materials take priority over the
/// built-in presets, falling back to `materials[0]` if `material_name` matches
/// neither. Shared by the live render loop and `export_thread::SceneSnapshot::capture`
/// so both pick a material the same way.
pub fn resolve_material(
    materials: &[GemMaterial],
    custom_materials: &[GemMaterial],
    material_name: &str,
) -> GemMaterial {
    custom_materials
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(material_name))
        .or_else(|| {
            materials
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(material_name))
        })
        .cloned()
        .unwrap_or_else(|| materials[0].clone())
}

/// Prefers `material_override` (see [`RenderContext::material_override`]) over the
/// plain by-name lookup [`resolve_material`] already does -- CAD audit items 57
/// and 61. Purely additive: [`resolve_material`]'s own signature and every
/// existing call site are untouched, so a caller that has no override to offer
/// (or hasn't been updated to look one up yet) keeps its exact prior behaviour by
/// passing `None`.
///
/// Callers outside `bridge::render_thread` that want a design's real effective
/// material honoured end to end (the tilt sweep, the tilt hover preview, a
/// high-resolution export) should switch their existing `resolve_material(...)`
/// call to `resolve_material_with_override(..., ctx.material_override.as_ref(),
/// ...)` -- see this crate's CAD audit notes for items 57/61 for the specific
/// call sites still on the old by-name-only path.
#[must_use]
pub fn resolve_material_with_override(
    materials: &[GemMaterial],
    custom_materials: &[GemMaterial],
    material_override: Option<&GemMaterial>,
    material_name: &str,
) -> GemMaterial {
    material_override
        .cloned()
        .unwrap_or_else(|| resolve_material(materials, custom_materials, material_name))
}

/// Every opt-in render-time material override bundled into one struct, to keep call
/// sites' argument lists short. Shared by the live render loop and
/// `export_thread::SceneSnapshot::capture`, which is what keeps a high-resolution
/// export from silently differing from the viewport it was taken from.
#[derive(Clone, Copy)]
pub struct MaterialOverrides {
    /// Inclusion/subsurface scattering: see `RenderContext::inclusion_sigma_s`.
    pub inclusion_sigma_s: f32,
    /// Crystal-axis orientation: see `RenderContext::c_axis_override`.
    pub c_axis_override: Option<Vec3>,
    /// Facet edge rounding: see `RenderContext::edge_rounding_radius`.
    pub edge_rounding_radius: f32,
    /// Physical stone size: see `RenderContext::stone_width_mm`.
    pub stone_width_mm: f32,
}

/// Applies every [`MaterialOverrides`] field on top of a resolved base material. Each
/// one is opt-in and skips its underlying `GemMaterial::with_*` call entirely at its
/// off position, so a material with nothing dialled in renders bit-identical to before
/// these controls existed.
///
/// `active_planes`/`width_cache` are only consulted for `stone_width_mm` -- passed in
/// rather than looked up internally so the live render loop can reuse one persistent
/// `StoneWidthCache` across frames while a one-shot caller can hand in a fresh one.
#[must_use]
pub fn apply_material_overrides(
    material: GemMaterial,
    overrides: &MaterialOverrides,
    active_planes: &[GpuFacetPlane],
    width_cache: &mut StoneWidthCache,
) -> GemMaterial {
    // Opt-in only: skipped entirely (not called with 0.0) at the off position.
    let material = if overrides.inclusion_sigma_s > 0.0 {
        material.with_scattering_amount(overrides.inclusion_sigma_s)
    } else {
        material
    };

    // An isotropic material's optic axis is physically meaningless (no birefringence
    // to orient). The settings-dialog control is disabled for one, but this guard is
    // what stops a leftover override from a previously selected anisotropic material
    // reaching an isotropic one's `c_axis`.
    let mut material = material;
    if let Some(axis) = overrides.c_axis_override
        && material.optical_character != OpticalCharacter::Isotropic
    {
        material.c_axis = axis;
    }

    let material = if overrides.edge_rounding_radius > 0.0 {
        material.with_edge_rounding(overrides.edge_rounding_radius)
    } else {
        material
    };

    // A degenerate/unmeasurable plane arrangement or a non-finite/non-positive scale
    // leaves the material untouched, rather than risking a NaN/negative path-length
    // multiplier reaching the tracer.
    if overrides.stone_width_mm > 0.0
        && let Some(model_width) = width_cache.ensure(active_planes)
        && model_width > 1e-9
    {
        let scale = (f64::from(overrides.stone_width_mm) / model_width) as f32;
        if scale.is_finite() && scale > 0.0 {
            return material.with_absorption_path_scale(scale);
        }
    }
    material
}

/// Resolves the current gem material (see `resolve_material_with_override`, which
/// prefers `material_override` over `material_name` when present -- CAD audit
/// items 57/61), applies every user material override on top of it (see
/// [`MaterialOverrides`]/[`apply_material_overrides`]), and derives this frame's
/// samples-per-frame from the user's target sample count.
///
/// Bounce count is not resolved here -- the settings dialog's "Max Ray Bounces"
/// selector is the only thing controlling it; callers use `RenderContext::max_bounces`
/// directly.
/// Everything needed to name the material for a frame: the two tables to look a
/// name up in, the name itself, and the editor's already-resolved override that
/// beats both when it is set (CAD audit items 57/61).
pub struct MaterialSources<'a> {
    pub materials: &'a [GemMaterial],
    pub custom_materials: &'a [GemMaterial],
    pub material_override: Option<&'a GemMaterial>,
    pub material_name: &'a str,
}

pub(super) fn resolve_material_and_quality(
    sources: &MaterialSources<'_>,
    target_samples: u32,
    overrides: &MaterialOverrides,
    active_planes: &[GpuFacetPlane],
    width_cache: &mut StoneWidthCache,
) -> (GemMaterial, u32) {
    let current_mat = resolve_material_with_override(
        sources.materials,
        sources.custom_materials,
        sources.material_override,
        sources.material_name,
    );
    let current_mat = apply_material_overrides(current_mat, overrides, active_planes, width_cache);

    // Samples-per-frame is derived from the target, not chosen directly: the render
    // loop (`render_thread::mod`) sleeps ~16ms per frame regardless of spp, so a large
    // target rendered at a fixed low spp would spend most of its wall-clock time
    // sleeping rather than tracing. Scaling spp with the target keeps that sleep
    // overhead roughly proportional instead of dominating at high targets.
    let spp = (target_samples / 64).clamp(1, 8);

    (current_mat, spp)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- PlanesOwner / claim_active_planes: CAD audit item 58 ------------------------

    #[test]
    fn builtin_is_the_default_owner_and_anything_may_claim_over_it() {
        let ctx = RenderContext::default();
        assert_eq!(ctx.planes_owner, PlanesOwner::Builtin);
        assert!(ctx.may_claim_active_planes(PlanesOwner::Catalogue { entry_id: 1 }));
        assert!(ctx.may_claim_active_planes(PlanesOwner::Editor { generation: 0 }));
    }

    #[test]
    fn a_catalogue_claim_never_overwrites_an_editor_owner() {
        let mut ctx = RenderContext::default();
        assert!(ctx.claim_active_planes(
            Arc::new(Vec::new()),
            None,
            PlanesOwner::Editor { generation: 3 },
        ));
        assert!(
            !ctx.may_claim_active_planes(PlanesOwner::Catalogue { entry_id: 42 }),
            "a catalogue click must not silently steal the slot from the editor \
             (the exact scenario CAD audit item 58 describes)"
        );
        assert_eq!(ctx.planes_owner, PlanesOwner::Editor { generation: 3 });
    }

    #[test]
    fn a_stale_editor_generation_never_overwrites_a_newer_one() {
        let mut ctx = RenderContext::default();
        assert!(ctx.claim_active_planes(
            Arc::new(Vec::new()),
            None,
            PlanesOwner::Editor { generation: 5 },
        ));
        // A background auto-solve started against generation 2 finishes late,
        // after a newer edit (generation 5) already landed -- it must not win.
        assert!(!ctx.claim_active_planes(
            Arc::new(Vec::new()),
            None,
            PlanesOwner::Editor { generation: 2 },
        ));
        assert_eq!(ctx.planes_owner, PlanesOwner::Editor { generation: 5 });
    }

    #[test]
    fn a_newer_or_equal_editor_generation_may_overwrite_the_current_one() {
        let mut ctx = RenderContext::default();
        assert!(ctx.claim_active_planes(
            Arc::new(Vec::new()),
            None,
            PlanesOwner::Editor { generation: 5 },
        ));
        assert!(ctx.claim_active_planes(
            Arc::new(Vec::new()),
            None,
            PlanesOwner::Editor { generation: 5 },
        ));
        assert!(ctx.claim_active_planes(
            Arc::new(Vec::new()),
            None,
            PlanesOwner::Editor { generation: 6 },
        ));
        assert_eq!(ctx.planes_owner, PlanesOwner::Editor { generation: 6 });
    }

    #[test]
    fn two_catalogue_claims_freely_replace_each_other() {
        let mut ctx = RenderContext::default();
        assert!(ctx.claim_active_planes(
            Arc::new(Vec::new()),
            None,
            PlanesOwner::Catalogue { entry_id: 1 },
        ));
        assert!(ctx.claim_active_planes(
            Arc::new(Vec::new()),
            None,
            PlanesOwner::Catalogue { entry_id: 2 },
        ));
        assert_eq!(ctx.planes_owner, PlanesOwner::Catalogue { entry_id: 2 });
    }

    // --- traced_planes_are_stale (CAD audit item 59) ---

    #[test]
    fn traced_planes_match_the_generation_they_were_claimed_at() {
        let mut ctx = RenderContext::default();
        assert!(ctx.claim_active_planes(
            Arc::new(Vec::new()),
            None,
            PlanesOwner::Editor { generation: 7 },
        ));
        assert!(!ctx.traced_planes_are_stale(7));
    }

    #[test]
    fn traced_planes_are_stale_once_a_newer_edit_has_landed() {
        let mut ctx = RenderContext::default();
        assert!(ctx.claim_active_planes(
            Arc::new(Vec::new()),
            None,
            PlanesOwner::Editor { generation: 7 },
        ));
        // An edit bumped `EditorState::generation` to 8, but nothing has
        // re-solved/re-claimed the slot yet -- the trace on screen is now for an
        // edit that no longer matches the live design.
        assert!(ctx.traced_planes_are_stale(8));
    }

    #[test]
    fn a_slot_the_editor_has_never_owned_is_never_stale() {
        let ctx = RenderContext::default();
        assert_eq!(ctx.planes_owner, PlanesOwner::Builtin);
        assert!(!ctx.traced_planes_are_stale(1));

        let mut ctx = RenderContext::default();
        assert!(ctx.claim_active_planes(
            Arc::new(Vec::new()),
            None,
            PlanesOwner::Catalogue { entry_id: 3 },
        ));
        assert!(!ctx.traced_planes_are_stale(1));
    }

    #[test]
    fn a_refused_claim_leaves_every_field_untouched() {
        let mut ctx = RenderContext::default();
        let original_planes = Arc::new(StandardGemCuts::emerald_cut());
        ctx.active_planes = Arc::clone(&original_planes);
        ctx.design_gear = Some((96, 0.5));
        ctx.planes_owner = PlanesOwner::Editor { generation: 10 };

        let accepted = ctx.claim_active_planes(
            Arc::new(StandardGemCuts::standard_round_brilliant()),
            Some((64, 0.0)),
            PlanesOwner::Catalogue { entry_id: 7 },
        );

        assert!(!accepted);
        assert!(Arc::ptr_eq(&ctx.active_planes, &original_planes));
        assert_eq!(ctx.design_gear, Some((96, 0.5)));
        assert_eq!(ctx.planes_owner, PlanesOwner::Editor { generation: 10 });
    }

    // ---- resolve_material_with_override: CAD audit items 57/61 -----------------------

    #[test]
    fn no_override_falls_through_to_the_plain_by_name_lookup() {
        let materials = GemMaterial::all_materials();
        let by_name = resolve_material(&materials, &[], "Diamond");
        let via_override_fn = resolve_material_with_override(&materials, &[], None, "Diamond");
        assert_eq!(by_name.name, via_override_fn.name);
        assert_eq!(by_name.dispersion, via_override_fn.dispersion);
    }

    #[test]
    fn an_override_wins_regardless_of_what_material_name_says() {
        let materials = GemMaterial::all_materials();
        let quartz = GemMaterial::new_custom("Quartz-ish", 1.5442, 0.013, 0.0, [0.0, 0.0, 0.0]);
        let resolved = resolve_material_with_override(
            &materials,
            &[],
            Some(&quartz),
            "Diamond", // the stale/fallback name a design with no real material carries
        );
        assert_eq!(resolved.name, quartz.name);
        assert_eq!(resolved.dispersion, quartz.dispersion);
    }
}
