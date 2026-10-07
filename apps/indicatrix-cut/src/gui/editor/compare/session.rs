//! The Slint-free half of the compare window: the two solved sides, the shared
//! orbit pose and its arithmetic, the status line, and the frame-generation book
//! that drops stale renders. Everything here is plain data plus pure functions, so
//! `super::tests` exercises it directly.

use crate::gui::{
    editor::material_lookup::{
        EditorMaterialLookup, resolved_gem_material, traced_gem_material, traced_material_for,
    },
    render::camera_lighting::{fit_distance_for_radius, orbit_distance_bounds},
    solid_preview::{
        cut_slider::cut_geometry,
        mesh_cache::{CachedMesh, MeshCache},
        preview_state::{CameraPose, DEFAULT_MESH_BOUNDING_RADIUS},
    },
};
use indicatrix::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use indicatrix_cut_core::Design;
use indicatrix_editor::optimize_view::default_optimize_material_ri;
use indicatrix_solid::preview::StoneGeometryBuf;
use std::f32::consts::FRAC_PI_2;

/// Radians of yaw/pitch per logical pixel of drag -- the same rate the main
/// viewports orbit at (`render::camera_lighting`'s `on_camera_orbit`), so turning
/// the stone here feels the same as turning it there.
pub(super) const ORBIT_RADIANS_PER_PX: f32 = 0.008;

/// Orbit distance per logical pixel of wheel delta -- the main viewports' own rate
/// (`render::camera_lighting`'s `on_camera_zoom`).
pub(super) const ZOOM_PER_WHEEL_PX: f32 = 0.002;

/// Which feature opened the window. Deliberately carries nothing that could commit
/// an edit: "Keep after" always goes back through the originating feature's own
/// Apply handler (see `super::wiring`), never through anything stored here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CompareOrigin {
    /// Retarget for material's pending proposal.
    Retarget,
    /// Optimize's pending (not yet applied) result.
    Optimize,
    /// The held design snapshot against the current design -- view only.
    Snapshot,
    /// Any two of the current design and its saved variants (the History tab's
    /// Variants view) -- view only.
    Variants,
}

impl CompareOrigin {
    /// Whether this origin has anything to keep or discard. A snapshot or variants
    /// comparison only ever looks: opening a variant is its own button on the list.
    #[must_use]
    pub(super) const fn offers_keep(self) -> bool {
        !matches!(self, Self::Snapshot | Self::Variants)
    }
}

/// Which renderer the window currently shows (`CompareModel.renderer_index`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Renderer {
    /// Flat grey solid raster -- instant, the default.
    Solid,
    /// The CPU path tracer at a modest fixed quality.
    Traced,
}

impl Renderer {
    /// `CompareModel.renderer_index` -> [`Renderer`]; anything but `1` is Solid.
    #[must_use]
    pub(super) const fn from_index(index: i32) -> Self {
        if index == 1 {
            Self::Traced
        } else {
            Self::Solid
        }
    }
}

/// Where the traced render for the CURRENT view stands, for the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TracedProgress {
    /// Waiting for the pose to settle (the 250 ms debounce after a drag).
    Waiting,
    /// Tracing side `side` of two (1 = before, 2 = after).
    Rendering {
        /// The side being traced right now, 1-based.
        side: u8,
    },
    /// Both sides traced at the current pose.
    Done,
}

/// One side's inputs, gathered on the UI thread before the solve moves off it.
pub(super) struct SideInput {
    /// The design to show on this side.
    pub(super) design: Design,
    /// The header/caption label, e.g. "Current design".
    pub(super) label: String,
    /// The material the traced mode renders this side in -- see
    /// [`resolve_side_material`].
    pub(super) material: Result<GemMaterial, String>,
    /// The material the optical figures are measured in -- see
    /// [`resolve_metrics_material`].
    pub(super) metrics_material: GemMaterial,
}

/// The material a side is traced as: the design's own traced material, resolved by
/// the same two functions the live viewport's material link uses
/// (`traced_material_for` + `traced_gem_material`, read-only), so a compared stone
/// traces exactly as it would in the live view. Refuses rather than substituting a
/// different species -- see `traced_material_for`'s own doc comment.
///
/// # Errors
///
/// `traced_material_for`'s own cutter-facing reason, or a short sentence when the
/// resolved name has no catalogue entry after all.
pub(super) fn resolve_side_material(
    design: &Design,
    custom: &[GemMaterial],
) -> Result<GemMaterial, String> {
    let (name, unresolved) = traced_material_for(design, custom);
    if let Some(reason) = unresolved {
        return Err(reason);
    }
    traced_gem_material(&name, &design.material, &EditorMaterialLookup::new(custom))
        .ok_or_else(|| format!("'{name}' is not a built-in preset or a saved custom material."))
}

/// The material a side's optical figures are measured in: the design's own material,
/// at the design's own refractive index when it names no species -- the defaulting
/// Optimize and the angle sweep apply, so the figures here agree with theirs.
///
/// Unlike [`resolve_side_material`] this never refuses: the figures of a design that
/// names no material still describe the stone at its refractive index, which is what
/// the cutter compares, whereas a picture in the wrong species would mislead.
#[must_use]
pub(super) fn resolve_metrics_material(design: &Design, custom: &[GemMaterial]) -> GemMaterial {
    let mut selection = design.material.clone();
    let _ = default_optimize_material_ri(design, &mut selection, custom);
    resolved_gem_material(&selection, &EditorMaterialLookup::new(custom))
}

/// One solved side of the comparison, built ONCE when the window opens. Keeps only
/// what rendering needs -- the solved masts, the stone (planes plus concave tools) both
/// renderers draw, and the traced material -- not the `Design` itself.
pub(super) struct CompareSide {
    /// The header/caption label.
    pub(super) label: String,
    /// The side's solved masts, `None` when it does not solve.
    pub(super) solved: Option<Vec<SolvedTier>>,
    /// The solve error, shown in the status line, when `solved` is `None`.
    pub(super) solve_error: Option<String>,
    /// The stone both renderers and the figures use: the half-space planes, preform
    /// planes first (the exact list `submit_design_ghost_preview` hands the solid
    /// worker), and the concave tools cut into them. Without concave tiers it is
    /// bit-identical to the planar stone (`tools` empty). Empty when the side does not
    /// solve.
    pub(super) stone: StoneGeometryBuf,
    /// How many leading entries of `stone.planes` are the rough's own preform planes
    /// (tinted in the solid view, like the Edit tab's Solid view does).
    pub(super) preform_planes: usize,
    /// The traced material, or why there is none.
    pub(super) material: Result<GemMaterial, String>,
    /// The material the optical figures are measured in.
    pub(super) metrics_material: GemMaterial,
    /// The solid's bounding radius, `None` when it does not close.
    pub(super) bounding_radius: Option<f64>,
    /// The design has concave tiers but none of them could be placed (an unresolvable
    /// set), so the flat stone is shown and measured instead.
    pub(super) tools_dropped: bool,
}

impl CompareSide {
    /// Solves `input.design` and derives its stone. Runs off the UI thread (see
    /// `super::wiring::open_compare`): a real design can take seconds to solve.
    #[must_use]
    pub(super) fn build(input: SideInput) -> Self {
        let preform_planes = input.design.preform.planes().len();
        match input.design.solve() {
            Ok(solved) => {
                // The one entry every redraw path uses; it reproduces
                // `design_to_gpu_planes_from_solved` bit for bit and adds the tools.
                let stone = cut_geometry(&input.design, Some(&solved), None);
                let tools_dropped =
                    !input.design.concave_tiers.is_empty() && stone.tools.is_empty();
                let bounding_radius = MeshCache::default()
                    .get_or_build_geometry(&stone)
                    .map(CachedMesh::bounding_radius);
                Self {
                    label: input.label,
                    solved: Some(solved),
                    solve_error: None,
                    stone,
                    preform_planes,
                    material: input.material,
                    metrics_material: input.metrics_material,
                    bounding_radius,
                    tools_dropped,
                }
            }
            Err(error) => Self {
                label: input.label,
                solved: None,
                solve_error: Some(error.to_string()),
                stone: StoneGeometryBuf::default(),
                preform_planes,
                material: input.material,
                metrics_material: input.metrics_material,
                bounding_radius: None,
                tools_dropped: false,
            },
        }
    }

    /// Whether this side solved (and so has geometry to draw).
    #[must_use]
    pub(super) const fn is_solved(&self) -> bool {
        self.solved.is_some()
    }
}

/// Both sides plus the shared camera. `pose` is the ONE pose both sides always
/// render at; `radius` sizes its zoom clamp to the larger of the two solids.
pub(super) struct CompareSession {
    /// The "before" side.
    pub(super) before: CompareSide,
    /// The "after" side.
    pub(super) after: CompareSide,
    /// Which feature opened the window.
    pub(super) origin: CompareOrigin,
    /// The shared orbit camera.
    pub(super) pose: CameraPose,
    /// The larger bounding radius of the two solids (the default radius when
    /// neither closes).
    pub(super) radius: f64,
}

impl CompareSession {
    /// Solves both sides and clamps `pose` into the new session's own zoom range.
    #[must_use]
    pub(super) fn build(
        before: SideInput,
        after: SideInput,
        origin: CompareOrigin,
        pose: CameraPose,
    ) -> Self {
        let before = CompareSide::build(before);
        let after = CompareSide::build(after);
        let radius = before
            .bounding_radius
            .into_iter()
            .chain(after.bounding_radius)
            .reduce(f64::max)
            .unwrap_or(DEFAULT_MESH_BOUNDING_RADIUS);
        let pose = fitted_pose(pose, radius);
        Self {
            before,
            after,
            origin,
            pose,
            radius,
        }
    }

    /// Whether "Keep after" may be offered: only for an origin that has something
    /// to keep, and only when BOTH sides solved -- keeping a change whose result
    /// (or whose baseline) cannot even be shown would be keeping it blind.
    #[must_use]
    pub(super) const fn can_keep(&self) -> bool {
        self.origin.offers_keep() && self.before.is_solved() && self.after.is_solved()
    }
}

/// The pose a session starts at: `pose` with its pitch clamped and its distance
/// pulled out to at least the "Fit" distance of `radius` (the LARGER of the two
/// sides' bounding radii), then clamped into the zoom range. The incoming pose is
/// the live viewport's, framed for the current design only, so without this a
/// proposal that comes out wider than the original would start clipped; a pose
/// already zoomed further out is kept.
#[must_use]
pub(super) fn fitted_pose(pose: CameraPose, radius: f64) -> CameraPose {
    let (min, max) = orbit_distance_bounds(radius);
    CameraPose {
        yaw: pose.yaw,
        pitch: clamp_pitch(pose.pitch),
        distance: pose
            .distance
            .max(fit_distance_for_radius(radius))
            .clamp(min, max),
    }
}

/// Whether a traced pair may be started: never while the mouse button is held on
/// the image (an orbit drag in progress). The instant solid pair and the difference
/// overlay follow the drag live; the traced pair waits for the release (plus its
/// debounce) or for the last wheel tick.
#[must_use]
pub(super) const fn traced_may_start(drag_held: bool) -> bool {
    !drag_held
}

/// How long a traced pair waits again for a layout switch's new slot size.
pub(super) const SIZE_SETTLE_POLL: std::time::Duration = std::time::Duration::from_millis(100);

/// How often a traced pair waits for the new slot size after a layout switch before
/// tracing anyway (a layout that kept the old size sends no size push).
pub(super) const MAX_SETTLE_POLLS: u8 = 3;

/// Whether the traced pair must wait once more for the slot size of a layout switch
/// (`size_settling`, with `polls` waits already spent) rather than trace at the old size.
#[must_use]
pub(super) const fn traced_waits_for_layout(size_settling: bool, polls: u8) -> bool {
    size_settling && polls < MAX_SETTLE_POLLS
}

/// Clamps an orbit pitch to straight down/straight up. Unlike the main viewport
/// (which wraps over the poles), the compare camera stops at them: two stones
/// turned upside-down past a pole are harder to compare than two held still.
#[must_use]
pub(super) const fn clamp_pitch(pitch: f32) -> f32 {
    pitch.clamp(-FRAC_PI_2, FRAC_PI_2)
}

/// `pose` orbited by a `(dx, dy)` drag, in logical pixels -- horizontal inverted,
/// vertical not, exactly like the main viewports (see `on_camera_orbit`'s own
/// comment for why the two axes want opposite signs). Pitch clamps at the poles.
#[must_use]
pub(super) fn orbit(pose: CameraPose, dx: f32, dy: f32) -> CameraPose {
    CameraPose {
        yaw: dx.mul_add(-ORBIT_RADIANS_PER_PX, pose.yaw),
        pitch: clamp_pitch(dy.mul_add(ORBIT_RADIANS_PER_PX, pose.pitch)),
        distance: pose.distance,
    }
}

/// `pose` zoomed by a wheel `delta` (logical pixels), clamped to
/// `orbit_distance_bounds(radius)` -- the main viewports' own clamp, sized to the
/// solids actually shown.
#[must_use]
pub(super) fn zoom(pose: CameraPose, delta: f32, radius: f64) -> CameraPose {
    let (min, max) = orbit_distance_bounds(radius);
    CameraPose {
        distance: delta
            .mul_add(-ZOOM_PER_WHEEL_PX, pose.distance)
            .clamp(min, max),
        ..pose
    }
}

/// The double-click reset: the app's "Front" view (yaw 0, pitch 0 -- the same pose
/// the viewports' own Front pill sets) at the "Fit" distance for `radius`.
#[must_use]
pub(super) fn default_pose(radius: f64) -> CameraPose {
    let (min, max) = orbit_distance_bounds(radius);
    CameraPose {
        yaw: 0.0,
        pitch: 0.0,
        distance: fit_distance_for_radius(radius).clamp(min, max),
    }
}

/// The split divider's fraction clamped to `[0, 1]`; `NaN` (a zero-width layout
/// divided through) falls back to the centre.
#[must_use]
pub(super) const fn clamp_split_fraction(fraction: f32) -> f32 {
    if fraction.is_nan() {
        0.5
    } else {
        fraction.clamp(0.0, 1.0)
    }
}

/// The dimmed status line under the images.
#[must_use]
pub(super) fn status_text(
    session: Option<&CompareSession>,
    renderer: Renderer,
    traced: TracedProgress,
    traced_spp: u32,
) -> String {
    let Some(session) = session else {
        return "Solving both sides…".to_string();
    };
    let mut parts: Vec<String> = Vec::new();
    for (name, side) in [("Before", &session.before), ("After", &session.after)] {
        if let Some(error) = &side.solve_error {
            parts.push(format!("{name} does not solve: {error}"));
        } else if renderer == Renderer::Traced && side.material.is_err() {
            parts.push(format!("{name}: material not resolved, nothing to trace"));
        } else if side.tools_dropped {
            parts.push(format!(
                "{name}: its concave tiers could not be placed, so the flat stone is shown"
            ));
        }
    }
    let mode = match (renderer, traced) {
        (Renderer::Solid, _) => "Solid".to_string(),
        (Renderer::Traced, TracedProgress::Waiting) => "Tracing…".to_string(),
        (Renderer::Traced, TracedProgress::Rendering { side }) => {
            format!("Tracing… {side} of 2")
        }
        (Renderer::Traced, TracedProgress::Done) => format!("Traced ({traced_spp} spp)"),
    };
    parts.insert(0, mode);
    parts.join("  ·  ")
}

/// A traced side that took longer than this many seconds gets fewer samples on the next
/// frame, when the stone carries concave tools (owner decision D2): the tool-aware
/// tracer is slower than the planar one, and an orbit that waits for it is a bad orbit.
pub(super) const SLOW_TRACED_SIDE_SECS: f64 = 3.0;

/// The samples per pixel a traced side gets: `full` for a planar stone, and for a stone
/// with concave tools too unless its previous trace took longer than
/// [`SLOW_TRACED_SIDE_SECS`], in which case half (never fewer than one).
#[must_use]
pub(super) fn traced_spp_for(full: u32, has_tools: bool, last_secs: Option<f64>) -> u32 {
    match last_secs {
        Some(secs) if has_tools && secs > SLOW_TRACED_SIDE_SECS => (full / 2).max(1),
        _ => full,
    }
}

/// The status-line addition when a traced side ran with fewer samples than `full`;
/// `None` when both sides had the full count.
#[must_use]
pub(super) fn reduced_spp_note(spp: [u32; 2], full: u32) -> Option<String> {
    let names = ["Before", "After"];
    let parts: Vec<String> = names
        .iter()
        .zip(spp)
        .filter(|&(_, used)| used < full)
        .map(|(name, used)| format!("{name} {used} spp (slow concave stone)"))
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// Which renderer produced a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FrameKind {
    /// A solid raster pair.
    Solid,
    /// A traced pair.
    Traced,
}

/// Request-generation bookkeeping: every view change (pose, size, renderer) bumps
/// the generation, every render request carries the generation it was issued at,
/// and a finished frame is shown only when it still describes the current view.
///
/// Within one generation a traced frame outranks a solid one: the solid pair lands
/// first (instantly) and the traced pair replaces it once ready, but a late solid
/// pair for the same view must never paint over a traced one already shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct FrameBook {
    generation: u64,
    traced_shown: bool,
}

impl FrameBook {
    /// Starts a new view and returns its generation.
    pub(super) const fn bump(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.traced_shown = false;
        self.generation
    }

    /// The current view's generation.
    #[must_use]
    pub(super) const fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether a finished `kind` frame issued at `generation` should be shown, given
    /// the renderer currently selected; records a shown traced frame.
    pub(super) const fn accept(
        &mut self,
        kind: FrameKind,
        generation: u64,
        renderer: Renderer,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        match kind {
            FrameKind::Solid => !self.traced_shown,
            FrameKind::Traced => {
                if !matches!(renderer, Renderer::Traced) {
                    return false;
                }
                self.traced_shown = true;
                true
            }
        }
    }
}
