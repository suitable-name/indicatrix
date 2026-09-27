//! The `RenderContext`/`FrameInputs` state: the shared, live-mutated render
//! configuration the GUI writes into and the render loop reads a per-frame snapshot
//! from ([`frame`]), plus material resolution and per-frame quality derivation
//! ([`materials`]).

mod frame;
mod materials;
pub mod scene_identity;
#[cfg(test)]
mod tests;

pub use frame::load_env_map;
pub(super) use frame::{FrameInputs, snapshot_frame_inputs};
pub(super) use materials::resolve_material_and_quality;
pub use materials::{
    MaterialOverrides, MaterialSources, apply_material_overrides, resolve_material,
    resolve_material_with_override,
};
use scene_identity::SceneIdentity;

use crate::{
    bridge::sample_cursor::LiveEpoch,
    settings::model::{LiveComputeTarget, LocalComputeTarget, LocalPreviewScale},
};
use glam::Vec3;
use indicatrix::{
    geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane},
    optics::{materials::GemMaterial, raytracer::LightingPreset},
    renderer::env_map::EnvironmentMap,
};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

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
    /// `material_name` in [`resolve_material_with_override`]. Plain by-name lookup
    /// cannot represent designs with no material name or unlisted RI overrides,
    /// which would fall back to Diamond. The intended writer is
    /// `gui::editor::material_lookup::resolved_gem_material`. `None` (default)
    /// reproduces by-name-only resolution.
    pub material_override: Option<GemMaterial>,
    pub lighting_preset: LightingPreset,
    /// What the camera sees behind the stone -- `AppSettings::backdrop`.
    pub backdrop: crate::settings::model::Backdrop,
    /// Progressive-accumulation target -- the single global target for the live image:
    /// the render loop stops once local plus (while combining) the live epoch's remote
    /// samples reach this, and a settled epoch's shared cursor covers exactly
    /// `[0, target_samples)`. Samples-per-frame is derived from it, not chosen directly
    /// -- see `resolve_material_and_quality`.
    pub target_samples: u32,
    pub max_bounces: u32,
    pub exposure: f32,
    /// Inclusion/subsurface scattering amount, applied via
    /// `GemMaterial::with_scattering_amount`. `0.0` (default) is off; useful range is
    /// `0.05` (barely perceptible) to `3.0` (milky) -- see `scattering_sigma_s`.
    pub inclusion_sigma_s: f32,
    /// Crystal-axis orientation override: `Some(axis)` replaces the resolved
    /// material's `c_axis` (skipped for isotropic materials); `None` (default, "as
    /// cut") leaves it untouched. Already resolved to a `Vec3` --
    /// `gui::optics::c_axis::angles_to_c_axis` converts the settings dialog's
    /// tilt/azimuth sliders here.
    pub c_axis_override: Option<Vec3>,
    /// Bruted (frosted) girdle finish toggle: `true` renders the identified girdle
    /// band as `FacetFinish::Frosted` instead of `Polished` -- see
    /// `GirdleFinishCache`. `false` (default) is the all-polished path.
    pub girdle_frosted: bool,
    /// Facet edge (meet-point) rounding radius, via `GemMaterial::with_edge_rounding`.
    /// `0.0` (default) is off (sharp edges). See `edge_rounding_radius` in
    /// `crates/indicatrix/src/optics/materials/mod.rs` for units/range.
    pub edge_rounding_radius: f32,
    /// Physical stone size: girdle width in mm for absorption/scattering. `0.0`
    /// (default) is off. When positive, scales by typed mm divided by the design's
    /// measured girdle width, so the value persists across `active_planes` owner
    /// changes (catalogue browse, editor solve completion) and can scale for a
    /// different stone. See [`Self::claim_active_planes`], which logs this case.
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
    /// Which of `active_planes`'s four writers most recently claimed the slot.
    /// Currently records the claim only; enforcement is partial. See
    /// [`Self::claim_active_planes`] for the intended single point of mutation.
    pub planes_owner: PlanesOwner,
    /// Why the design cannot be traced honestly, as a cutter-facing message.
    /// `None` when tracing is valid. Set when a design has no material name and
    /// its refractive index matches no built-in preset; refusal to substitute is
    /// the chosen behaviour over silent fallback to Diamond.
    pub material_unresolved: Option<String>,
    pub custom_materials: Arc<Vec<GemMaterial>>,
    /// Name -> specific gravity for custom catalogue materials. A parallel side
    /// channel to [`Self::custom_materials`], not on `GemMaterial` itself (SG is
    /// gemological data unrelated to optical tracing). Kept in lock-step by
    /// `gui::optics::custom_materials`'s callbacks and `gui::mod`'s startup load.
    /// `Arc<Vec<..>>` for cheap cloning into per-frame snapshots.
    pub custom_material_specific_gravity: Arc<Vec<(String, f64)>>,
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
    /// Set by the remote-rendering orchestrator while a settled epoch's remote work
    /// owns (part of) the displayed image. Distinct from `paused`: driven by the
    /// handoff state machine, not the user, and never observable as a user pause.
    ///
    /// What `true` does depends on `live_compute_target`: for
    /// [`LiveComputeTarget::RemoteOnly`] it suspends local tracing (see
    /// `SuspensionFlags`); for [`LiveComputeTarget::Both`] local keeps tracing,
    /// claiming its per-frame ranges from [`live_epoch`](Self::live_epoch), and the
    /// display folds the epoch's remote contribution into the shown image (see
    /// `should_combine_remote`).
    ///
    /// Stays `true` past a completed remote contribution: the settled image keeps
    /// combining. Cleared only through [`Self::release_remote`] (every discard path:
    /// drag, failure in `RemoteOnly`, a compute-target change) or the render loop's own
    /// `resolve_remote_ownership` release on a non-drag scene change.
    pub remote_active: bool,
    /// The current settle's shared sample budget and merged remote contribution --
    /// see `bridge::sample_cursor::live`. Set together with `remote_active`/`dirty`
    /// in one locked mutation by the orchestrator's `start_remote_render`; cleared
    /// together with `remote_active` by [`Self::release_remote`]. Never carried from
    /// one epoch into the next: a new settle always installs a fresh one.
    ///
    /// `Arc`: the same instance the orchestrator's remote lane merges finished chunks
    /// into; the render thread only claims ranges from it and reads its sums.
    pub live_epoch: Option<Arc<LiveEpoch>>,
    /// A COUNT, not a `bool`, of the high-resolution render jobs currently in flight --
    /// the batch preview, the batch tilt-curve sweep, and a fanned-out hi-res export
    /// queue each increment this on start and decrement it on every exit path (success,
    /// error, cancel, or an unwinding `Drop` guard). A single shared `bool` would
    /// let one job's own cleanup re-enable local tracing while a SECOND job (e.g. a
    /// tilt batch finishing mid-export) is still running. Suspends local tracing like
    /// `paused`/`tab_visible`/`remote_active` (and contending for `GpuBackend`'s shared
    /// `Mutex`) whenever this is above zero -- see [`Self::export_active`] for the
    /// `bool` view every read site still wants. Must never be observable as a
    /// user-visible pause -- flipping `paused` instead would corrupt its restore.
    pub export_active_count: u32,
    /// Live rendering's Local/Remote/Local+Remote choice -- see
    /// `LiveComputeTarget`. Read fresh by `orchestrator::poll_tick` at every settle
    /// (so a change applies on the NEXT settle) and every frame by the render loop.
    pub live_compute_target: LiveComputeTarget,
    /// Final-picture live transfer: the current epoch's remote request asks for finished,
    /// denoised display frames (`TransferMode::DisplayOnly`) instead of float deltas.
    /// 8-bit frames cannot be merged with local samples, so while this holds a `Both`
    /// epoch behaves exactly like `RemoteOnly` (see [`Self::effective_live_target`]).
    /// Set by the orchestrator's `start_remote_render` in the same locked mutation as
    /// `remote_active`/`live_epoch`, cleared with them by [`Self::clear_remote_state`].
    pub live_display_only: bool,
    /// Local live-rendering CPU/CPU+GPU/GPU choice -- see `LocalComputeTarget`. Read
    /// fresh every frame by `accumulate_frame_samples`, so a settings change applies
    /// immediately with no restart. `CpuGpu` (default) is the hybrid
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
    /// The content-derived scene generation counter behind
    /// [`Self::scene_generation`] -- see [`scene_identity`]. Never written directly;
    /// `Default::default()` is the only sensible initial value.
    pub scene_identity: SceneIdentity,
}

/// Tags which subsystem last claimed `RenderContext::active_planes`/`design_gear`.
/// See [`RenderContext::claim_active_planes`] for the
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
    /// A catalogue entry, tagged by its row id so a delete-reset can
    /// check whether the row it just deleted is the one that owns the slot before
    /// resetting it.
    Catalogue {
        /// The catalogue row's database id.
        entry_id: i64,
    },
}

impl RenderContext {
    /// Whether a write tagged `new_owner` may overwrite the slot's CURRENT owner --
    /// the arbitration rule, factored out so every writer
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
    /// `planes_owner` together. Returns `false` (and leaves
    /// every field untouched) when [`Self::may_claim_active_planes`] refuses the
    /// claim, so a caller can decide whether to surface that as a toast/prompt.
    ///
    /// Does NOT set `dirty` -- callers already do that themselves alongside
    /// whatever else a plane-set change requires (a `Reproject`/`Replan` request,
    /// a tilt-sweep re-request, etc.), and folding it in here would
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
        // A new owner is about to take over `active_planes`,
        // and `stone_width_mm` (never itself reset by a plane-slot change -- see
        // that field's own doc comment) is still on. Nothing here can tell
        // whether the figure was actually meant for THIS new owner or is simply
        // left over from whichever design was active when it was typed, so this
        // only logs -- an outright reset would just as often clear a value the
        // cutter deliberately wants to keep applying (e.g. re-measuring the same
        // physical rough across two catalogue rows).
        if self.stone_width_mm > 0.0 && self.planes_owner != owner {
            tracing::warn!(
                stone_width_mm = self.stone_width_mm,
                previous_owner = ?self.planes_owner,
                new_owner = ?owner,
                "active_planes' owner is changing while stone_width_mm is still set; \
                 absorption/scattering scale may now be computed against the wrong \
                 stone's measured width"
            );
        }
        self.active_planes = planes;
        self.design_gear = design_gear;
        self.planes_owner = owner;
        true
    }

    /// Whether the currently traced/rasterized `active_planes`
    /// describe an OLDER edit than `current_generation` -- the "render is stale"
    /// signal. No `planes_generation`/`trace_stale` property exists anywhere else in
    /// the app.
    /// [`PlanesOwner::Editor`] already carries exactly the generation
    /// `active_planes` was last claimed at, so this is a
    /// direct comparison against it rather than a new field -- `false` whenever
    /// the slot is not even owned by the editor (`Builtin`/`Catalogue`), since
    /// "stale relative to an edit" only means something while the editor owns the
    /// slot at all.
    ///
    /// This alone does not surface staleness to the cutter: a caller still needs to call
    /// it with `EditorState::generation`'s live value and push the result into a new
    /// Slint property with an amber overlay in the Path-traced/Both viewport (`ui/**`).
    #[must_use]
    pub const fn traced_planes_are_stale(&self, current_generation: u64) -> bool {
        matches!(
            self.planes_owner,
            PlanesOwner::Editor { generation } if generation != current_generation
        )
    }

    /// Looks up `name`'s specific gravity in [`Self::custom_material_specific_gravity`],
    /// case-insensitively -- the same matching convention every
    /// other name lookup this struct's fields feed uses (see [`resolve_material`]).
    /// `None` when no custom material by that name has a recorded SG.
    #[must_use]
    pub fn custom_specific_gravity(&self, name: &str) -> Option<f64> {
        self.custom_material_specific_gravity
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, sg)| *sg)
    }

    /// Whether ANY high-resolution render job currently suspends local tracing -- the
    /// `bool` view of [`Self::export_active_count`] every existing reader wants.
    #[must_use]
    pub const fn export_active(&self) -> bool {
        self.export_active_count > 0
    }

    /// Releases every piece of live remote state together -- `remote_active` and the
    /// epoch (its cursor, remote sums and in-flight chunk accumulator all live inside
    /// [`LiveEpoch`]) -- and marks the frame `dirty` so local tracing restarts from a
    /// clean buffer. The one function every discard path calls (drag, a failure in
    /// `RemoteOnly`, a live-compute-target change): local samples claimed from the
    /// released epoch's cursor are not a prefix, so continuing to accumulate on top of
    /// them with the plain `accum_samples` offset would re-trace indices.
    pub fn release_remote(&mut self) {
        self.clear_remote_state();
        self.dirty = true;
    }

    /// [`Self::release_remote`] without touching `dirty` -- for the render loop's own
    /// release, which happens on a frame that is ALREADY resetting its accumulation
    /// because of the `dirty` it just consumed (setting it again would throw away one
    /// more frame for nothing, and clearing it could swallow a callback's fresh one).
    pub fn clear_remote_state(&mut self) {
        self.remote_active = false;
        self.live_epoch = None;
        self.live_display_only = false;
    }

    /// The live compute target the render loop and the orchestrator actually act on:
    /// [`Self::live_compute_target`], except that a final-picture epoch
    /// ([`Self::live_display_only`]) turns `Both` into `RemoteOnly` -- local tracing
    /// pauses after the handoff and the remote's display frames are the image.
    #[must_use]
    pub const fn effective_live_target(&self) -> LiveComputeTarget {
        effective_live_target(self.live_compute_target, self.live_display_only)
    }

    /// Locks `ctx`, recovering from a poisoned mutex rather than panicking -- the one
    /// convention every `RenderContext` lock in this crate should follow,
    /// since a guard here only ever wraps plain field reads/writes with no partial-
    /// update invariant a poisoning panic could have left broken. An `.unwrap()` call
    /// site instead would let one panic while the lock is held poison it
    /// for every later caller, turning an unrelated later click, drag or window close
    /// into a second, unrelated UI-thread panic.
    pub fn lock(ctx: &Arc<Mutex<Self>>) -> MutexGuard<'_, Self> {
        ctx.lock().unwrap_or_else(PoisonError::into_inner)
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
            custom_material_specific_gravity: Arc::new(Vec::new()),
            running: true,
            dirty: true,
            paused: false,
            tab_visible: true,
            denoise_enabled: true,
            remote_active: false,
            live_epoch: None,
            export_active_count: 0,
            live_compute_target: LiveComputeTarget::Both,
            live_display_only: false,
            local_compute_target: LocalComputeTarget::CpuGpu,
            local_preview_scale: LocalPreviewScale::Off,
            camera_moving: false,
            env_map: None,
            scene_identity: SceneIdentity::default(),
        }
    }
}

/// [`RenderContext::effective_live_target`]'s pure rule: a final-picture epoch
/// (`display_only`) cannot combine 8-bit remote frames with local samples, so `Both`
/// acts as `RemoteOnly`; every other combination is unchanged.
#[must_use]
pub const fn effective_live_target(
    target: LiveComputeTarget,
    display_only: bool,
) -> LiveComputeTarget {
    match (target, display_only) {
        (LiveComputeTarget::Both, true) => LiveComputeTarget::RemoteOnly,
        (other, _) => other,
    }
}
