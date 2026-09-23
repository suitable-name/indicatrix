// Only `search` is `pub`: `refresh_diagram_list` is reused by a downstream binary's
// sync-complete handler (see `search::refresh_diagram_list`'s doc comment). The rest
// are wiring `build_main_window` uses internally.
mod batch;
// The `indicatrix-cut-core`-backed Edit sub-tab.
mod editor;
// The Edit sub-tab's resizable-dock/collapsible-section layout persistence -- lives
// in its own module purely to keep this file from growing further; see this
// module's own doc comment.
mod editor_layout;
mod library;
mod optics;
// The single shared 5x7 bitmap-font glyph lookup for `solid_preview::diagram2d`'s
// panel labels and `tilt::video_export::overlay`'s readout text -- see the
// module's own doc comment.
pub(in crate::gui) mod pixel_font;
// The one place the `rfd` crate's file dialog is ever constructed in this app
// -- an off-UI-thread native picker/save-as/pick-folder worker plus the
// UI-thread rendezvous and test hook every other module's own picker needs
// use through this. See the module's own doc comment.
mod pickers;
mod remote;
mod render;
pub mod solid_preview;
// Startup settings-application and the settings-value <-> UI-pill-index conversions
// that go with it -- moved out of this file purely to keep it from growing further.
// Several of its items are used well beyond startup (e.g. by callbacks that need the
// same index<->value mapping later), so this module re-exports them flatly below
// rather than requiring every caller to spell out `gui::startup_settings::`.
mod startup_settings;
mod tilt;
// Fits the main window to its monitor on first show -- see the module doc for why the
// .slint preferred size alone is not enough on Full HD displays.
mod window_sizing;

// `sync_range_bounds_to_ui` is defined in `library::diagram_list` but re-exported here
// so its public path (`gui::sync_range_bounds_to_ui`) is unchanged for a downstream
// binary's sync-complete handler and this crate's own `gui::library`, both of which
// call it at that spelling.
pub use library::diagram_list::sync_range_bounds_to_ui;
// `search` is grouped into `gui::library` with the rest of the library UI (see
// that module's own doc comment), and re-exported here under this name so
// `gui::search` (the path a downstream binary's sync-complete handler is
// documented to use -- see `search::refresh_diagram_list`'s doc comment) keeps
// resolving.
pub use library::search;
use startup_settings::apply_loaded_settings;
// Flat re-exports so every existing call site elsewhere in `gui` (which all spell
// these as `gui::X`, never `gui::startup_settings::X`) keeps resolving unchanged.
pub(in crate::gui) use startup_settings::{
    color_space_from_index, env_map_status_text, is_c_axis_override_available,
    local_compute_target_from_index, local_preview_scale_from_index,
    refresh_lighting_preset_options, refresh_material_options,
};

use crate::{
    EditorModel, LibraryModel, MainWindow, RemoteWorkerModel, SettingsModel, SolidPreviewModel,
    TiltModel, ViewportModel,
    bridge::{
        library::source::LibrarySource,
        render_thread::{PlanesOwner, RenderContext, spawn_render_thread},
    },
    gui::{
        optics::{
            crystal_optics::{custom_material_specific_gravity_from_rows, gem_material_from_row},
            curve_path::tilt_curve_path,
        },
        remote::{refresh_worker_options, setup_remote_rendering, setup_worker_callbacks},
        solid_preview::{
            diagram_wiring::{self, DiagramFacetTier, DiagramHoverText, DiagramPick},
            preview_state::{
                DEFAULT_MESH_BOUNDING_RADIUS, PickBuffer, PreviewFrame, PreviewSink,
                SolidLastSolved, SolidPickState, SolidPreviewState,
            },
        },
    },
    settings::{self, SettingsPersister},
};
use indicatrix::{geometry::plane::GpuFacetPlane, optics::materials::GemMaterial};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Weak};
use std::sync::{Arc, Mutex};

const DB_PATH: &str = "facet_diagrams.sqlite";

/// This crate's binary entry point (`apps/indicatrix-cut/src/main.rs` calls
/// straight through to this).
///
/// # Errors
///
/// See [`run_gui`].
pub fn main() -> anyhow::Result<()> {
    run_gui()
}

/// A fully constructed and wired [`MainWindow`], not yet shown or running. Built by
/// [`build_main_window`].
///
/// Exists (rather than `run_gui` doing everything and blocking on `ui.run()`) so a
/// second binary that wants this *same*, already-wired public window -- the private
/// edition's GUI, which shows this window alongside its own separate sync window --
/// can call [`build_main_window`], `.show()` both windows, and drive a single shared
/// event loop itself, without duplicating any of this crate's rendering/settings/
/// remote-rendering wiring.
///
/// `ui` is the only public field. The rest (render context, settings persister,
/// remote-rendering poll timer) exist purely to be kept alive for the life of the
/// window -- dropping any of them early would stop the thing it drives (the render
/// thread, settings autosave, remote-rendering handoff polling) -- so the caller only
/// needs to keep the whole `MainWindowHandle` alive, never reach into its internals.
pub struct MainWindowHandle {
    pub ui: MainWindow,
    // `render_ctx`/`remote_rendering_timer` are never read again after construction --
    // both exist purely so dropping `MainWindowHandle` is what stops the render thread/
    // remote-rendering handoff polling, not so anything downstream can inspect them.
    // `#[expect(dead_code)]` rather than removing them: removing a field would drop it
    // at the end of `build_main_window` instead of at the end of the caller's
    // `MainWindowHandle`'s lifetime, which is the entire point of holding onto them
    // here. `settings_store` is the same kind of RAII guard (keeps the debounced
    // settings writer alive) but is also read once, by `run_gui` right below, hence no
    // `#[expect(dead_code)]` on it -- see that field's own comment.
    #[expect(
        dead_code,
        reason = "RAII guard field -- kept alive, never read; see comment above"
    )]
    render_ctx: Arc<Mutex<RenderContext>>,
    // Not `#[expect(dead_code)]` like its two siblings above/below: `run_gui` reads
    // this field directly (`handle.settings_store.snapshot()`) to learn whether the
    // Edit sub-tab's layout was ever touched, for `window_sizing::
    // fit_initial_window_size`'s small-screen default. Still exists primarily as the
    // same kind of RAII guard -- kept alive so the debounced settings writer keeps
    // running for the life of the window -- but, unlike its siblings, it is also
    // read directly by `run_gui`, per the paragraph above.
    settings_store: Arc<SettingsPersister>,
    #[expect(
        dead_code,
        reason = "RAII guard field -- kept alive, never read; see comment above"
    )]
    remote_rendering_timer: slint::Timer,
}

/// Runs this crate's window standalone: builds it (see [`build_main_window`])
/// and runs it to completion.
///
/// # Errors
///
/// Returns an error if [`build_main_window`] or the window's own event loop
/// (`MainWindow::run`) fails -- see [`build_main_window`]'s doc comment for what that
/// covers.
pub fn run_gui() -> anyhow::Result<()> {
    let handle = build_main_window()?;
    // Spelled out instead of `MainWindow::run` so the monitor fit can be queued
    // between `show` and the event loop (the winit window only exists once the loop
    // turns -- see `window_sizing`).
    handle.ui.show()?;
    // Whether the user has ever touched the Edit sub-tab's layout -- read from the
    // settings store's own in-memory snapshot (already seeded from disk by
    // `editor_layout::apply_editor_layout_from_settings` above) rather than the UI's
    // `EditorModel.dock_width`/etc. directly, since `fit_initial_window_size` only
    // needs this one flag, not the whole layout. See `window_sizing`'s own doc
    // comment for what it does with it.
    let editor_layout_touched = handle
        .settings_store
        .snapshot()
        .settings
        .editor_layout_touched;
    window_sizing::fit_initial_window_size(&handle.ui, editor_layout_touched);
    slint::run_event_loop()?;
    handle.ui.hide()?;
    Ok(())
}

/// The real [`PreviewSink`] -- hops back to the UI thread exactly like
/// `bridge::render_thread::frame_helpers::push_frame_to_ui` already does for the
/// path-traced view, pushing a finished solid-preview frame into `MainWindow`'s
/// `editor_solid_image`/`editor_has_solid`/`editor_solid_status`/`editor_solid_stale`/
/// `editor_solid_edges_image`/`editor_has_solid_edges`/`editor_solid_diagram_image`/
/// `editor_has_diagram` properties. See `gui::solid_preview::preview_state`'s own
/// module doc comment ("`PreviewSink`: kept generic over Slint on purpose") for why
/// this hop lives here rather than inside `SolidPreviewState` itself.
///
/// `pick`/`last_solved`/`diagram_pick`/`diagram_hover_text`/`diagram_facet_tier` are
/// plain `Arc<Mutex<..>>` state, not Slint properties, so `gui::editor`'s hover/click
/// callbacks, `solid_preview::diagram_wiring`'s Diagram-mode hover/click callbacks,
/// and the next edit's [`solid_preview::preview_state::ReplanRequest::last_solved`]
/// can all read them without needing a Slint window handle of their own. See
/// `preview_state`'s own module doc comment ("Where `last_solved` lives") for why
/// these live here rather than on `gui::editor::state::EditorState`.
///
/// They are stored inside [`PreviewSink::apply`]'s `upgrade_in_event_loop` closure,
/// on the UI thread, together with the image swap that closure already performs --
/// NOT immediately on the calling worker thread. See that `impl`'s own doc comment
/// ("Atomicity guarantee") for why: a click landing between an immediate worker-
/// thread store and a deferred image swap would resolve against a pick buffer
/// newer than the image still on screen -- see `docs/history/indicatrix-cut.md`.
struct SlintSolidSink {
    ui: Weak<MainWindow>,
    pick: Arc<Mutex<Option<PickBuffer>>>,
    last_solved: SolidLastSolved,
    // Diagram mode's (view_mode 3) own pick buffer/hover-text/tier tables -- a
    // separate `Arc<Mutex<..>>` triple from `pick`/`last_solved` above, since the
    // diagram is an entirely different pixel layout resolved without ever needing
    // `Design`/`FacetMap` access on the UI thread (see
    // `solid_preview::diagram_wiring`'s own module doc comment).
    diagram_pick: DiagramPick,
    diagram_hover_text: DiagramHoverText,
    diagram_facet_tier: DiagramFacetTier,
    /// The index wheel's own per-pixel tooth-picking buffer
    /// (`PreviewFrame::diagram_tooth_pick`) -- a separate buffer from
    /// `diagram_pick` above, same reasoning (the diagram's pixel layout has no
    /// relation to the Solid rasterizer's own pick buffer). Reuses the
    /// `DiagramPick` type alias since it is exactly the same shape
    /// (`Arc<Mutex<Option<PickBuffer>>>`).
    diagram_tooth_pick: DiagramPick,
    /// The Solid view's own facet id -> hover-tooltip-text table, stored
    /// alongside `pick` from every frame's [`PreviewFrame::hover_text`] so a future
    /// `gui::editor::callbacks::tier_actions` hover/click handler can index it
    /// instead of rebuilding a `facet_map::FacetMap` per mouse move. See this
    /// field's `build_main_window` construction site (`solid_hover_text`) for the
    /// handoff this sets up but does not finish (that indexing change lives
    /// elsewhere in `gui::editor`).
    hover_text: Arc<Mutex<Vec<String>>>,
    /// The Solid view's own facet id -> owning tier index table, from every
    /// frame's [`PreviewFrame::facet_tier`] -- see `hover_text`'s doc comment.
    facet_tier: Arc<Mutex<Vec<Option<usize>>>>,
    /// The shared render context.
    ///
    /// A finished frame's own [`PreviewFrame::planes`] are published back into
    /// `RenderContext::active_planes` here (only when they actually differ from
    /// what is already there), so a camera orbit right after an in-budget live
    /// edit re-issues the just-edited geometry instead of snapping back to
    /// whatever `RenderContext` last held -- see [`PreviewFrame::planes`]'s own
    /// doc comment for the full mechanism.
    ///
    /// Also read (via `active_planes`'s `Arc` pointer identity) to decide
    /// whether this frame's own geometry still matches the path tracer's
    /// last-pushed one -- see `solid_active_planes`/`trace_active_planes` below.
    render_ctx: Arc<Mutex<RenderContext>>,
    /// The plane arrangement (`RenderContext::active_planes`, by `Arc`
    /// pointer identity) this sink last published a solid-preview frame with.
    /// Compared against `trace_active_planes` (written by the path tracer's own
    /// frame-push closure in [`build_main_window`]) to keep
    /// `SolidPreviewModel.trace_matches_solid` current -- see
    /// [`planes_generations_match`].
    solid_active_planes: Arc<Mutex<Option<Arc<Vec<GpuFacetPlane>>>>>,
    /// The path tracer's own last-pushed plane arrangement -- see
    /// `solid_active_planes`'s doc comment.
    trace_active_planes: Arc<Mutex<Option<Arc<Vec<GpuFacetPlane>>>>>,
    /// The current solid's own bounding radius
    /// (`PreviewFrame::mesh_bounding_radius`), stashed here so
    /// `render::camera_lighting`'s orbit-zoom clamp and "Fit" pose can read the
    /// design actually loaded instead of a fixed range. Read by `camera_lighting::
    /// setup_camera_and_lighting_callbacks`'s own copy of this `Arc`, written
    /// only here.
    mesh_bounding_radius: Arc<Mutex<f64>>,
}

/// Whether the LAST path-traced frame and the LAST solid-preview frame were
/// both produced from the exact same plane arrangement, compared by `Arc` pointer
/// identity of `RenderContext::active_planes` at the moment each was pushed --
/// never by value, since two DIFFERENT designs can coincidentally solve to
/// identical planes, and a cheap pointer compare is all either frame-push closure
/// can afford to do on the UI thread on every frame.
///
/// `true` whenever either side has not published a frame yet -- nothing to
/// disagree with, matching `SolidPreviewModel.trace_matches_solid`'s own
/// documented default.
fn planes_generations_match(
    trace: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
    solid: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
) -> bool {
    let trace = trace
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let solid = solid
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match (trace, solid) {
        (Some(trace), Some(solid)) => Arc::ptr_eq(&trace, &solid),
        _ => true,
    }
}

/// Publishes `planes` into `render_ctx`'s `active_planes` when they actually
/// differ from what is already there (skipped entirely when `planes` is empty, so
/// this never clobbers `RenderContext::default`'s own placeholder cut), stashes
/// the resulting (possibly unchanged) arrangement into `solid_active_planes`, and
/// returns whether it now agrees with `trace_active_planes` (via
/// [`planes_generations_match`]). Split out of [`SlintSolidSink::apply`] purely to
/// keep that function under clippy's function-length lint -- see
/// [`PreviewFrame::planes`]'s doc comment for the full mechanism and
/// [`SlintSolidSink::solid_active_planes`]'s doc comment for the plane-arrangement
/// comparison this feeds.
///
/// A write here goes through `RenderContext::claim_active_planes` tagged
/// `PlanesOwner::Editor { generation }`, the same arbitration
/// `editor::auto_solve::apply_background_solve_result` already uses, so a solid-preview
/// frame that is behind the live design (a slow replan for design A still finishing
/// after New/Load switched to design B) can never clobber a newer claim's planes --
/// the same generation check that `editor::apply_matching_preview_frame` (called
/// just above this function's call site, on the SAME frame's `generation`) applies
/// to the tier table also applies here to `active_planes`. A frame whose
/// `generation` is `0` -- no `Replan` has ever landed for the live design, see
/// [`PreviewFrame::generation`]'s own doc comment -- is never published at all.
fn sync_planes_and_check_trace_match(
    planes: &[(glam::Vec3, f32)],
    render_ctx: &Mutex<RenderContext>,
    solid_active_planes: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
    trace_active_planes: &Mutex<Option<Arc<Vec<GpuFacetPlane>>>>,
    generation: u64,
) -> bool {
    if !planes.is_empty() {
        let converted: Vec<GpuFacetPlane> = planes
            .iter()
            .map(|&(normal, offset)| GpuFacetPlane::new(normal, -offset))
            .collect();
        let mut ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if generation != 0 && ctx.active_planes.as_ref() != &converted {
            let design_gear = ctx.design_gear;
            if ctx.claim_active_planes(
                Arc::new(converted),
                design_gear,
                PlanesOwner::Editor { generation },
            ) {
                ctx.dirty = true;
            }
        }
        *solid_active_planes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::clone(&ctx.active_planes));
    }
    planes_generations_match(trace_active_planes, solid_active_planes)
}

/// Stores `value` into `state` when `Some`, leaving `state` untouched for `None`
/// -- the Diagram-mode side tables (`PreviewFrame::diagram_pick`/
/// `diagram_tooth_pick`/`diagram_hover_text`/`diagram_facet_tier`) are only ever
/// `Some` together, for a `view_mode`-3 request (see `PreviewFrame::
/// diagram_hover_text`'s own doc comment). Split out of
/// [`SlintSolidSink::apply`] purely to keep that function under clippy's
/// function-length lint: four separate four-line `if let` blocks collapse to one
/// call each.
fn store_if_some<T>(state: &Mutex<Option<T>>, value: Option<T>) {
    if let Some(value) = value {
        *state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(value);
    }
}

impl PreviewSink for SlintSolidSink {
    /// # Atomicity guarantee
    ///
    /// Every piece of this frame's state -- `pick`/`last_solved`/the Diagram-mode
    /// side tables AND the displayed image -- is swapped together, inside this one
    /// `upgrade_in_event_loop` closure, on the UI thread. Storing the pick buffers
    /// immediately on the WORKER thread while deferring only the image swap to the
    /// UI thread's event loop would open a window where a click landing on the OLD
    /// (still-displayed) image resolves against the NEW pick buffer already stashed
    /// for the next frame, picking the wrong facet.
    /// Deferring every store into the closure closes that window: whichever frame's
    /// image is on screen, that SAME frame's pick/solved/diagram data is what a click
    /// arriving right after the swap will see -- never a newer buffer paired with an
    /// older image, or vice versa.
    fn apply(&self, frame: PreviewFrame) {
        let PreviewFrame {
            image,
            has_solid,
            status,
            solved,
            stale,
            pick,
            edges_image,
            diagram_image,
            has_diagram,
            diagram_pick,
            diagram_tooth_pick,
            diagram_hover_text,
            diagram_facet_tier,
            planes,
            hover_text,
            facet_tier,
            generation,
            mesh_bounding_radius,
        } = frame;

        // Cloned Arcs (cheap), not `self` -- the closure below outlives this call.
        let pick_state = Arc::clone(&self.pick);
        let last_solved_state = Arc::clone(&self.last_solved);
        let diagram_pick_state = Arc::clone(&self.diagram_pick);
        let diagram_tooth_pick_state = Arc::clone(&self.diagram_tooth_pick);
        let diagram_hover_text_state = Arc::clone(&self.diagram_hover_text);
        let diagram_facet_tier_state = Arc::clone(&self.diagram_facet_tier);
        let hover_text_state = Arc::clone(&self.hover_text);
        let facet_tier_state = Arc::clone(&self.facet_tier);
        let render_ctx_state = Arc::clone(&self.render_ctx);
        let solid_active_planes_state = Arc::clone(&self.solid_active_planes);
        let trace_active_planes_state = Arc::clone(&self.trace_active_planes);
        let mesh_bounding_radius_state = Arc::clone(&self.mesh_bounding_radius);

        // `slint::Image::from_rgba8` runs HERE, not on the worker thread -- see
        // `gui::solid_preview::to_pixel_buffer`'s doc comment (`slint::Image` is not
        // `Send`).
        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            *pick_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pick);
            // Kept alongside the image/pick swap in this same atomicity
            // boundary -- see this function's own doc comment -- so the orbit
            // camera's distance clamp (`render::camera_lighting`) never reads a
            // radius that describes a different frame than the one on screen.
            *mesh_bounding_radius_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = mesh_bounding_radius;
            // Every replan frame's freshly solved masts land here, unconditionally,
            // on every edit -- including ones the tier table still painted "Not
            // solved" for, because `auto_solve`'s own debounced background solve
            // (`editor::auto_solve::on_edit`/`dispatch_background_solve`) is a
            // SEPARATE `Design::solve()` against the same design, racing this one.
            // `editor::apply_matching_preview_frame` resolves that race: when
            // `generation` still names the live design (see
            // `editor::auto_solve::take_matching_design`'s own doc comment for the
            // exact check -- a superseded frame, from a design an edit has since
            // moved past, is a deliberate no-op here), it pushes the tier table's
            // rows/status/warnings/yield figures straight from `solved` via
            // `editor::view::push_solved_preview`, AND cancels whatever debounced
            // auto-solve was about to recompute the exact same thing.
            // `solved.as_ref()` only borrows -- `solved` itself still moves into
            // `last_solved_state` below.
            if let Some(masts) = solved.as_ref() {
                editor::apply_matching_preview_frame(&ui, &render_ctx_state, generation, masts);
            }
            *last_solved_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = solved;
            // Diagram mode's side tables are only ever `Some` for a view_mode-3
            // request -- see `PreviewFrame::diagram_hover_text`'s doc comment.
            // `diagram_tooth_pick` shares that same "only `Some` together"
            // contract.
            store_if_some(&diagram_pick_state, diagram_pick);
            store_if_some(&diagram_tooth_pick_state, diagram_tooth_pick);
            store_if_some(&diagram_hover_text_state, diagram_hover_text);
            store_if_some(&diagram_facet_tier_state, diagram_facet_tier);
            // Unconditional (unlike the Diagram-only tables above) -- see
            // `SlintSolidSink::hover_text`'s doc comment.
            *hover_text_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = hover_text;
            *facet_tier_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = facet_tier;

            // See `sync_planes_and_check_trace_match`'s own doc comment. Also
            // passes this SAME frame's `generation` (already destructured above,
            // for `apply_matching_preview_frame`) so a stale frame's planes can be
            // told apart from the live design's.
            let trace_matches = sync_planes_and_check_trace_match(
                &planes,
                &render_ctx_state,
                &solid_active_planes_state,
                &trace_active_planes_state,
                generation,
            );
            ui.global::<SolidPreviewModel>()
                .set_trace_matches_solid(trace_matches);

            ui.global::<SolidPreviewModel>()
                .set_image(slint::Image::from_rgba8(image));
            ui.global::<SolidPreviewModel>().set_has_solid(has_solid);
            ui.global::<SolidPreviewModel>().set_status(status.into());
            ui.global::<SolidPreviewModel>().set_stale(stale);
            if let Some(edges_image) = edges_image {
                ui.global::<SolidPreviewModel>()
                    .set_edges_image(slint::Image::from_rgba8(edges_image));
                ui.global::<SolidPreviewModel>().set_has_solid_edges(true);
            } else {
                ui.global::<SolidPreviewModel>().set_has_solid_edges(false);
            }
            if let Some(diagram_image) = diagram_image {
                ui.global::<SolidPreviewModel>()
                    .set_diagram_image(slint::Image::from_rgba8(diagram_image));
            }
            ui.global::<SolidPreviewModel>()
                .set_has_diagram(has_diagram);
        });
    }
}

/// Converts one startup `get_custom_materials()` row read into BOTH
/// `RenderContext::custom_materials` and `RenderContext::
/// custom_material_specific_gravity` together -- pulled out
/// of [`build_main_window`] purely so this pure conversion is unit-testable
/// without constructing a whole `MainWindow`/`Database`.
///
/// Both lists are derived from the same row read so they can never disagree:
/// without deriving `custom_material_specific_gravity` here too,
/// `RenderContext::custom_material_specific_gravity` would stay empty until this
/// session's own save/delete callback (`gui::optics::custom_materials`) ran at
/// least once -- a custom material's carat-weight estimate would read blank for
/// the whole first browse of a freshly launched app, even for a material that
/// already had a recorded specific gravity on disk.
#[must_use]
fn initial_custom_materials_and_specific_gravity(
    rows: &[indicatrix_vault::model::material::CustomMaterialRow],
) -> (Vec<GemMaterial>, Vec<(String, f64)>) {
    let materials = rows.iter().map(gem_material_from_row).collect();
    let specific_gravity = custom_material_specific_gravity_from_rows(rows);
    (materials, specific_gravity)
}

/// Builds and fully wires a `MainWindow`, up to but not including actually
/// running the event loop. See [`MainWindowHandle`]'s doc comment for why this
/// is split out from [`run_gui`].
///
/// # Errors
///
/// Returns an error if constructing the `MainWindow` itself fails (a Slint platform
/// initialization failure), or if the on-disk design library fails to open AND the
/// in-memory fallback database also fails to open (SQLite itself unusable in this
/// process -- see the database-open block below). A plain on-disk open failure is
/// handled internally: it falls back to a throwaway in-memory database and reports
/// the failure via a persistent error toast rather than propagating it or panicking.
///
/// # Panics
///
/// Panics if any of this function's several `Mutex::lock().unwrap()` calls (`db`,
/// `render_ctx`) observes a poisoned lock -- which only happens if some earlier code
/// already panicked while holding that same lock, so this is a symptom of a prior
/// panic elsewhere, not a new failure mode this function introduces.
// Long by one function's worth of setup calls (every one of them already split out
// and individually under the limit) -- this is `run_gui`'s original body, unchanged
// in shape by the `MainWindowHandle` split; splitting it further would just move the
// line count into an equally long "call every setup_* function" wrapper.
#[expect(
    clippy::too_many_lines,
    reason = "a flat sequence of already-extracted setup_* calls (see comment above) -- \
              splitting further just moves the same line count into a wrapper that \
              calls them all, not a real reduction"
)]
pub fn build_main_window() -> anyhow::Result<MainWindowHandle> {
    let ui = MainWindow::new()?;

    // Local Compute (Feature: a local CPU/CPU+GPU/GPU switch): whether this build was
    // compiled with the `gpu` feature -- Rust-driven, set once here (Slint has no
    // `cfg!` of its own), never touched again. `settings_dialog.slint`'s "Local
    // Compute" row binds its whole `visible:` to this, so a CPU-only build never
    // offers a choice it has no GPU path to act on.
    ui.global::<SettingsModel>()
        .set_gpu_build(cfg!(feature = "gpu"));

    // The editor is not optional (there is no `editor` Cargo feature gating it),
    // so `EditorModel.enabled` is unconditionally `true` -- kept as a real
    // property (rather than deleted) because `app.slint`'s "Edit" sub-tab and
    // several other components still read it as their enablement guard. Slint
    // markup has no `#[cfg]` of its own, so this flag is how they would detect a
    // gated build if one ever existed.
    ui.global::<EditorModel>().set_enabled(true);

    // Connect to SQLite DB. If the on-disk database can't be opened (locked, corrupt,
    // unwritable directory, ...), fall back to a throwaway in-memory database rather
    // than panicking at startup -- `Database::new` runs the exact same
    // create-tables-if-not-exist + migration chain against `:memory:` as it does
    // against a real file (see `Database::new`'s own doc comment), so the fallback
    // yields a fully-functional, just non-persistent, library. The user is told via a
    // persistent error toast (never auto-dismisses -- see `show_toast`'s doc comment)
    // rather than a status-bar message, since a status bar can be set and then never
    // seen if a later step panics before the next redraw.
    let db = match Database::new(Some(DB_PATH)) {
        Ok(db_inst) => Arc::new(Mutex::new(db_inst)),
        Err(e) => {
            match Database::new(Some(":memory:")) {
                Ok(memory_db) => {
                    show_toast(
                        &ui,
                        &format!(
                            "Could not open the design library ({e}) -- running in a \
                             temporary, unsaved library for this session."
                        ),
                        "error",
                    );
                    Arc::new(Mutex::new(memory_db))
                }
                Err(memory_err) => {
                    // Both the real database AND an in-memory SQLite connection
                    // failed to open -- SQLite itself is unusable in this process
                    // (e.g. no SQLite support compiled in, or the process is out of
                    // memory). There is no further degraded mode to fall back to, so
                    // report both errors and bail out of window construction instead
                    // of panicking.
                    return Err(anyhow::anyhow!(
                        "failed to open design library at {DB_PATH:?} ({e}), and the \
                         in-memory fallback database also failed to open ({memory_err})"
                    ));
                }
            }
        }
    };

    // Shared Render Context for 3D Viewport
    let render_ctx = Arc::new(Mutex::new(RenderContext::default()));

    // The Edit tab's solid-inspection preview controller (see
    // `gui::solid_preview::preview_state`'s own module doc comment). Built here,
    // alongside `render_ctx` above -- `setup_camera_and_lighting_callbacks` below
    // needs it unconditionally (the Live Render viewport's own orbit/zoom drag also
    // re-issues the last solid planes at the new pose). Its worker thread is spawned
    // lazily on the first `request_redraw`, so a session that never opens the Edit
    // tab's Solid view never pays for one.
    //
    // The last rendered frame's per-pixel facet-picking buffer, and the
    // most-recent-resolved-mast cache -- both written by `SlintSolidSink::apply` (the
    // worker thread's callback) and read by `gui::editor`'s hover/click callbacks and
    // edit callbacks respectively. See `solid_preview::preview_state`'s own module doc
    // comment ("Where `last_solved` lives") for why these live here, as plain
    // `Arc<Mutex<..>>`s, rather than on `EditorState` itself.
    let solid_pick: Arc<Mutex<Option<PickBuffer>>> = Arc::new(Mutex::new(None));
    let solid_last_solved: SolidLastSolved = Arc::new(Mutex::new(None));
    // Diagram mode's (view_mode 3) own pick buffer/hover-text/tier tables -- see
    // `SlintSolidSink`'s own doc comment for why these are separate from
    // `solid_pick`/`solid_last_solved` above.
    let diagram_pick: DiagramPick = Arc::new(Mutex::new(None));
    // The index wheel's own tooth pick buffer -- same shape/lifetime as
    // `diagram_pick` above, see `SlintSolidSink::diagram_tooth_pick`'s own doc
    // comment.
    let diagram_tooth_pick: DiagramPick = Arc::new(Mutex::new(None));
    let diagram_hover_text: DiagramHoverText = Arc::new(Mutex::new(None));
    let diagram_facet_tier: DiagramFacetTier = Arc::new(Mutex::new(None));
    // The Solid view's own facet id -> hover-text/owning-tier tables, from
    // every frame's `PreviewFrame::hover_text`/`facet_tier` -- the Solid-mode
    // counterpart to `diagram_hover_text`/`diagram_facet_tier` above. Not yet
    // threaded into `editor::setup_editor_callbacks` below (that call site would
    // need two more parameters, and the hover/click handlers that would index
    // these live in `gui::editor::callbacks::tier_actions`) -- see
    // `SlintSolidSink::hover_text`'s doc comment for the full handoff.
    let solid_hover_text: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let solid_facet_tier: Arc<Mutex<Vec<Option<usize>>>> = Arc::new(Mutex::new(Vec::new()));
    // The path tracer's and the solid worker's own last-published plane
    // arrangements (by `Arc` pointer identity) -- see `planes_generations_match`'s
    // doc comment. `trace_active_planes` is also written from the render thread's
    // own frame-push closure below.
    let solid_active_planes: Arc<Mutex<Option<Arc<Vec<GpuFacetPlane>>>>> =
        Arc::new(Mutex::new(None));
    let trace_active_planes: Arc<Mutex<Option<Arc<Vec<GpuFacetPlane>>>>> =
        Arc::new(Mutex::new(None));
    // The current solid's own bounding radius, written by every frame
    // (`SlintSolidSink::apply`) and read by `render::camera_lighting`'s orbit-zoom
    // clamp and "Fit" pose -- see `SlintSolidSink::mesh_bounding_radius`'s own doc
    // comment. Defaults to the same fallback a frame carries before anything has
    // ever closed.
    let mesh_bounding_radius: Arc<Mutex<f64>> = Arc::new(Mutex::new(DEFAULT_MESH_BOUNDING_RADIUS));
    let solid_preview_state = SolidPreviewState::new(Arc::new(SlintSolidSink {
        ui: ui.as_weak(),
        pick: Arc::clone(&solid_pick),
        last_solved: Arc::clone(&solid_last_solved),
        diagram_pick: Arc::clone(&diagram_pick),
        diagram_tooth_pick: Arc::clone(&diagram_tooth_pick),
        diagram_hover_text: Arc::clone(&diagram_hover_text),
        diagram_facet_tier: Arc::clone(&diagram_facet_tier),
        hover_text: Arc::clone(&solid_hover_text),
        facet_tier: Arc::clone(&solid_facet_tier),
        render_ctx: Arc::clone(&render_ctx),
        solid_active_planes: Arc::clone(&solid_active_planes),
        trace_active_planes: Arc::clone(&trace_active_planes),
        mesh_bounding_radius: Arc::clone(&mesh_bounding_radius),
    }));
    // Diagram mode's own hover/click callbacks -- see
    // `solid_preview::diagram_wiring`'s own module doc comment for why this needs
    // no `gui::editor`/`Design` access, unlike the ordinary Solid view's hover/
    // click callbacks (`gui::editor::callbacks::tier_actions::
    // setup_solid_facet_hover_callback`/`setup_solid_facet_click_callback`).
    // `&solid_preview_state` lets the diagram's own hover/click
    // drive `SolidPreviewState::request_facet_overlay` -- see that function's own
    // doc comment for what each callback now does with it.
    diagram_wiring::setup_diagram_hover_and_click_callbacks(
        &ui,
        &diagram_pick,
        &diagram_tooth_pick,
        &diagram_hover_text,
        &diagram_facet_tier,
        &solid_preview_state,
    );

    // Which library (local database, or a remote worker) the viewer is
    // currently browsing. Starts `LibrarySource::Local` -- see that type's own doc
    // comment on why every local-only call site keeps behaving as plain local-only
    // until a user actually switches.
    let library_source: Arc<Mutex<LibrarySource>> = Arc::new(Mutex::new(LibrarySource::default()));

    // Load custom gemstone materials from SQLite. `indicatrix-vault` returns plain
    // `CustomMaterialRow`s (it must not depend on `indicatrix`), so convert them into
    // `indicatrix::optics::materials::GemMaterial` here at the boundary.
    let custom_material_rows = db
        .lock()
        .unwrap()
        .get_custom_materials()
        .unwrap_or_default();
    let (initial_custom_mats, initial_custom_sg) =
        initial_custom_materials_and_specific_gravity(&custom_material_rows);
    {
        let mut ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ctx.custom_materials = Arc::new(initial_custom_mats.clone());
        ctx.custom_material_specific_gravity = Arc::new(initial_custom_sg);
    }
    refresh_material_options(&ui, &initial_custom_mats);

    // Settings persistence + lighting presets. `load_or_default` never fails/panics --
    // a missing, unreadable, or corrupt file just logs and yields defaults. Applied
    // into `render_ctx` and the UI's mirrored properties before the render thread
    // starts, then handed to a debounced background writer that every
    // settings-changing callback below feeds.
    let settings_path = settings::store::default_settings_path();
    // One-time carry-over from the public `indicatrix-cut`'s settings file -- must run
    // before `load_or_default` reads `settings_path`, and only ever copies into a
    // genuinely absent file. See `migrate_legacy_settings_if_needed`'s own doc comment
    // for the copy-never-move/never-overwrite/never-blocks-startup guarantees.
    settings::store::migrate_legacy_settings_if_needed(&settings_path);
    let loaded_settings = settings::store::load_or_default(&settings_path);
    apply_loaded_settings(&ui, &render_ctx, &loaded_settings);
    // The Edit sub-tab's resizable dock width / inspector height / collapsed
    // sections -- see `editor_layout::apply_editor_layout_from_settings`'s own doc
    // comment for why this runs before `setup_editor_layout_callbacks` below.
    editor_layout::apply_editor_layout_from_settings(&ui, &loaded_settings.settings);
    refresh_lighting_preset_options(&ui, &loaded_settings.presets);
    // Remote rendering: the worker-list panel's rows, restored from the last saved
    // configuration.
    refresh_worker_options(&ui, &loaded_settings.settings.remote_workers);
    ui.global::<RemoteWorkerModel>()
        .set_denoise_enabled(loaded_settings.settings.denoise_enabled);
    let settings_store = Arc::new(SettingsPersister::spawn(settings_path, loaded_settings));

    // Detachable Live Render window: sub-tab/detach-toggle wiring, plus the shared
    // frame-routing target the render thread's own frame-push closure below also
    // mirrors into -- see `detached_render`'s module doc comment.
    let detached_frame_target = render::detached_render::setup_live_render_visibility_callbacks(
        &ui,
        &render_ctx,
        &solid_preview_state,
    );
    // The settings load above (`apply_loaded_settings`/`apply_loaded_ui_mirrors`)
    // already pushed the restored `ViewportModel.live_view_mode`/`SolidPreviewModel.
    // view_mode` into the UI, but nothing had recomputed `render_ctx.tab_visible`
    // against them yet -- it was still sitting at `RenderContext::default()`'s `true`.
    // Without this, a session restored into Solid mode would keep tracing (wasting
    // GPU/remote time, the very thing this gate exists to stop) until the user
    // happened to touch a tab. One-time sync, now that every signal
    // `detached_render::render_is_visible` reads is in place.
    render::detached_render::recompute_tab_visible(&ui, &render_ctx);

    // Spawn Background Multi-Threaded Physically Based Spectral Raytracer
    let ui_weak_render = ui.as_weak();
    let render_ctx_frame_push = render_ctx.clone();
    let trace_active_planes_push = Arc::clone(&trace_active_planes);
    let solid_active_planes_push = Arc::clone(&solid_active_planes);
    spawn_render_thread(
        ui_weak_render,
        render_ctx.clone(),
        move |ui: &MainWindow, img: slint::SharedPixelBuffer<slint::Rgba8Pixel>| {
            // Both destinations are updated on EVERY frame regardless of which one is
            // currently visible -- see `detached_render`'s module doc comment's "Frame
            // routing" section for why that's what keeps either side of a dock/undock
            // from ever showing a stale or frozen frame. `slint::Image::from_rgba8`
            // wraps `img` in a cheap, atomically-refcounted handle, so cloning it for
            // the (usually absent) second destination costs a pointer bump, not a
            // pixel copy.
            let slint_img = slint::Image::from_rgba8(img);
            ui.global::<ViewportModel>()
                .set_render_image(slint_img.clone());
            ui.global::<ViewportModel>().set_has_render(true);
            // The lock guard is dropped (via this `let`) before the `if let` below --
            // holding it across the scrutinee would keep it alive for the whole `if`
            // body, including the `set_render_image`/`set_has_render` calls, which
            // don't need it at all.
            let detached_weak = detached_frame_target.lock().unwrap().clone();
            if let Some(detached) = detached_weak.and_then(|weak| weak.upgrade()) {
                detached.set_render_image(slint_img);
                detached.set_has_render(true);
            }
            // This path-traced frame reflects whatever `RenderContext::
            // active_planes` currently is -- remember it (by `Arc` pointer
            // identity) and recompute `SolidPreviewModel.trace_matches_solid`
            // against the solid worker's own last-published arrangement. Already
            // running on the UI thread (this closure is `spawn_render_thread`'s
            // `update_image` callback, itself only ever invoked via
            // `Weak::upgrade_in_event_loop` -- see `bridge::render_thread::
            // frame_helpers::push_frame_to_ui`), so locking `render_ctx` here is
            // exactly as safe as every other UI-thread callback in this file
            // already locking it.
            let planes_now = render_ctx_frame_push
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .active_planes
                .clone();
            *trace_active_planes_push
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(planes_now);
            ui.global::<SolidPreviewModel>()
                .set_trace_matches_solid(planes_generations_match(
                    &trace_active_planes_push,
                    &solid_active_planes_push,
                ));
        },
        |ui: &MainWindow,
         brilliance: f32,
         fire: f32,
         scintillation: f32,
         windowing: f32,
         extinction: f32,
         graph_brilliance: [f32; 19],
         graph_extinction: [f32; 19],
         graph_windowing: [f32; 19],
         cam_pitch_deg: f32| {
            ui.global::<ViewportModel>().set_brilliance_pct(brilliance);
            ui.global::<ViewportModel>().set_fire_index(fire);
            ui.global::<ViewportModel>()
                .set_scintillation_pct(scintillation);
            ui.global::<ViewportModel>().set_windowing_pct(windowing);
            ui.global::<ViewportModel>().set_extinction_pct(extinction);
            ui.global::<TiltModel>()
                .set_graph_brilliance(slint::ModelRc::new(slint::VecModel::from(
                    graph_brilliance.to_vec(),
                )));
            ui.global::<TiltModel>()
                .set_graph_extinction(slint::ModelRc::new(slint::VecModel::from(
                    graph_extinction.to_vec(),
                )));
            ui.global::<TiltModel>()
                .set_graph_windowing(slint::ModelRc::new(slint::VecModel::from(
                    graph_windowing.to_vec(),
                )));
            // Tilt-curve `Path::commands` strings for the performance graph dialog's
            // line chart -- derived here rather than threading three more arguments
            // through this closure (see `curve_path::tilt_curve_path`'s doc comment).
            ui.global::<TiltModel>()
                .set_graph_brilliance_path(tilt_curve_path(&graph_brilliance).into());
            ui.global::<TiltModel>()
                .set_graph_extinction_path(tilt_curve_path(&graph_extinction).into());
            ui.global::<TiltModel>()
                .set_graph_windowing_path(tilt_curve_path(&graph_windowing).into());
            ui.global::<TiltModel>().set_cam_pitch_deg(cam_pitch_deg);
        },
        // Pushes `ViewportGpu`'s self-healing status text
        // (empty while healthy) to `ViewportModel.gpu_status_text` -- see
        // `spawn_render_thread`'s own `update_gpu_status` doc comment.
        |ui: &MainWindow, text: slint::SharedString| {
            ui.global::<ViewportModel>().set_gpu_status_text(text);
        },
    );

    // Persist the library panel's collapsed/expanded state -- same "debounced
    // background writer" persistence every other durable setting in this file already
    // goes through, just a one-off `bool` with no dedicated `gui::*` module of its own.
    let settings_store_panel = settings_store.clone();
    ui.global::<LibraryModel>()
        .on_panel_collapsed_changed(move |collapsed: bool| {
            settings_store_panel.update(|s| s.settings.library_panel_collapsed = collapsed);
        });
    // Persist the Edit sub-tab's resizable dock width / inspector height /
    // collapsed sections the same way -- see `editor_layout::
    // setup_editor_layout_callbacks`'s own doc comment.
    editor_layout::setup_editor_layout_callbacks(&ui, &settings_store);

    setup_user_manual_callback(&ui);
    setup_reveal_last_saved_callback(&ui);
    render::camera_lighting::setup_camera_and_lighting_callbacks(
        &ui,
        &render_ctx,
        &settings_store,
        &solid_preview_state,
        &mesh_bounding_radius,
    );
    render::material_quality::setup_material_changed_callback(&ui, &render_ctx, &settings_store);
    render::material_quality::setup_backdrop_callback(&ui, &render_ctx, &settings_store);
    render::material_quality::setup_material_and_quality_callbacks(
        &ui,
        &render_ctx,
        &settings_store,
    );
    render::material_quality::setup_material_effect_override_callbacks(
        &ui,
        &render_ctx,
        &settings_store,
    );
    render::camera_lighting::setup_environment_map_callbacks(&ui, &render_ctx, &settings_store);
    render::lighting_presets::setup_lighting_preset_callbacks(&ui, &render_ctx, &settings_store);
    render::render_export::setup_render_export_callbacks(&ui, &render_ctx, &settings_store);
    optics::custom_materials::setup_custom_material_callbacks(&ui, &render_ctx, &db);
    library::clipboard::setup_copy_callbacks(&ui);
    tilt::tilt_profile::setup_tilt_profile_callback(&ui, &render_ctx);
    tilt::tilt_hover_preview::setup_tilt_hover_preview_callback(&ui, &render_ctx);
    library::diagram_list::load_filter_options_and_initial_list(&ui, &db);
    library::diagram_list::setup_search_and_filter_callbacks(&ui, &db, &library_source);
    // The owner's "regenerate library previews/tilt curves for the filtered set"
    // request -- see `setup_regenerate_filtered_set_callbacks`'s own doc comment.
    library::diagram_list::setup_regenerate_filtered_set_callbacks(&ui, &db, &library_source);
    library::diagram_list::setup_diagram_selection_and_export_callbacks(
        &ui,
        &db,
        &library_source,
        &render_ctx,
        &solid_preview_state,
    );
    // Tilt-performance filter rows (add/remove/clear-all) -- Rust-mediated but
    // UI-owned state.
    library::diagram_list::setup_performance_filter_callbacks(&ui);
    library::local::setup_import_callback(&ui, &db, &library_source, &settings_store);
    library::local::setup_rename_callback(&ui, &db, &library_source);
    library::local::setup_set_shape_callback(&ui, &db, &library_source);
    library::local::setup_delete_callback(&ui, &db, &library_source, &render_ctx);
    library::local::setup_export_asc_callback(&ui, &db, &library_source);
    // Per-design ignore/un-ignore toggle -- grouped with the other library CRUD
    // callbacks above since it's the same shape (local-only write, then
    // `refresh_after_library_change`).
    library::local::setup_ignore_toggle_callback(&ui, &db, &library_source);
    library::local::setup_add_tag_callback(&ui, &db, &library_source);
    library::local::setup_remove_tag_callback(&ui, &db, &library_source);
    library::detail::setup_save_metadata_callback(&ui, &db, &library_source, &render_ctx);
    // The cutting-table's per-mode row-count pills.
    library::detail::setup_filtered_row_count_callback(&ui);
    // The Edit sub-tab's own callbacks (tier add/remove/modify, preform controls,
    // undo/redo, loading the selected design, `.asc` export).
    // Bundles the three Solid-viewport handles above into the one value
    // `editor::setup_editor_callbacks` takes for them -- see [`SolidPickState`]'s
    // own doc comment for why they travel together.
    let solid_pick_state = SolidPickState {
        pick: Arc::clone(&solid_pick),
        hover_text: Arc::clone(&solid_hover_text),
        facet_tier: Arc::clone(&solid_facet_tier),
    };
    editor::setup_editor_callbacks(
        &ui,
        &db,
        &library_source,
        &render_ctx,
        &solid_preview_state,
        &solid_last_solved,
        &solid_pick_state,
    );
    // Persist the Solid viewport's view mode through the same debounced writer every
    // other durable setting in this file goes through -- see
    // `AppSettings::solid_view_mode`'s own doc comment. Also recomputes
    // `render_ctx.tab_visible`: this mode is one of the five signals
    // `render::detached_render::render_is_visible` reads (the Edit tab traces live
    // only in Path-traced/Both), so flipping it can turn the tracer on or off.
    //
    // Also re-requests a redraw at the new mode via `resubmit_at_current_pose` --
    // otherwise switching TO Diagram or Both would never ask the worker for
    // anything, so both would stay on whatever `has_diagram`/`has_solid_edges` a
    // PREVIOUS mode happened to leave set (usually `false`, showing the "Solve to
    // preview" placeholder until the user jiggled the mouse or made an edit).
    let settings_store_solid_view_mode = settings_store.clone();
    let render_ctx_solid_view_mode = render_ctx.clone();
    let preview_state_solid_view_mode = Arc::clone(&solid_preview_state);
    let ui_weak_solid_view_mode = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_view_mode_changed(move |mode: i32| {
            settings_store_solid_view_mode.update(|s| {
                s.settings.solid_view_mode = u8::try_from(mode).unwrap_or_default();
            });
            if let Some(ui) = ui_weak_solid_view_mode.upgrade() {
                render::detached_render::recompute_tab_visible(&ui, &render_ctx_solid_view_mode);
                let mut ctx = render_ctx_solid_view_mode
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Path-traced/Both (modes 1/2) must resume accumulation from
                // scratch on entry, mirroring `on_live_view_mode_changed` below --
                // otherwise an accumulator already at `target_samples` for the
                // current planes leaves the render thread's worker loop sleeping
                // instead of sending a display cycle (`bridge/render_thread/mod.rs`),
                // so flipping the Edit tab into Path-traced/Both would resume
                // tracing but never force a visible restart: the newly revealed
                // surface would show whatever was last pushed, reading as "this is
                // current" even when it isn't.
                if mode == 1 || mode == 2 {
                    ctx.dirty = true;
                }
                render::camera_lighting::resubmit_at_current_pose(
                    &ui,
                    &ctx,
                    &preview_state_solid_view_mode,
                );
                // Explicit: releases the `RenderContext` mutex right after its
                // last use rather than leaving it held until this block's
                // closing brace.
                drop(ctx);
            }
        });
    // Handles the debounced `SolidPreviewModel.viewport_resized()` signal
    // (`solid_viewport.slint`'s `resize_pulse` timer) -- without it, dragging the
    // dock splitter or maximising the window would leave the shown image
    // rescaled/letterboxed and the pick buffer misaligned from the cursor until the
    // next orbit/zoom redraw. Deferred by one event-loop turn (`slint::Timer::
    // single_shot`) so the resubmit reads the size AFTER Slint's own layout pass
    // for this resize has actually run, not whatever was current the instant the
    // debounce timer fired.
    let render_ctx_viewport_resized = render_ctx.clone();
    let preview_state_viewport_resized = Arc::clone(&solid_preview_state);
    let ui_weak_viewport_resized = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_viewport_resized(move || {
            let ui_weak = ui_weak_viewport_resized.clone();
            let render_ctx = render_ctx_viewport_resized.clone();
            let preview_state = Arc::clone(&preview_state_viewport_resized);
            slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                let ctx = render_ctx
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                render::camera_lighting::resubmit_at_current_pose(&ui, &ctx, &preview_state);
            });
        });
    // Persist the Live Render tab's own view mode the same way -- see
    // `AppSettings::live_view_mode`'s own doc comment. Same `recompute_tab_visible`
    // reasoning as `solid_view_mode` above (the Live tab traces live only in
    // Path-traced mode), plus two effects specific to this toggle:
    // - Flipping TO Path-traced (`1`) must resume accumulation from the current
    //   planes rather than showing whatever sample count was left over from the last
    //   time this mode was visible (`ctx.dirty = true` forces a fresh restart, same as
    //   every other scene-invalidating write to `RenderContext`).
    // - Flipping TO Solid (`0`) must show the solid raster immediately, not wait for
    //   the next camera drag or library selection to trigger a redraw
    //   (`camera_lighting::resubmit_live_solid`).
    let settings_store_live_view_mode = settings_store.clone();
    let render_ctx_live_view_mode = render_ctx.clone();
    let preview_state_live_view_mode = Arc::clone(&solid_preview_state);
    let ui_weak_live_view_mode = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_live_view_mode_changed(move |mode: i32| {
            settings_store_live_view_mode.update(|s| {
                s.settings.live_view_mode = u8::try_from(mode).unwrap_or_default();
            });
            let Some(ui) = ui_weak_live_view_mode.upgrade() else {
                return;
            };
            render::detached_render::recompute_tab_visible(&ui, &render_ctx_live_view_mode);
            let mut ctx = render_ctx_live_view_mode
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if mode == 1 {
                ctx.dirty = true;
            } else if mode == 0 {
                render::camera_lighting::resubmit_live_solid(
                    &ui,
                    &ctx,
                    &preview_state_live_view_mode,
                );
            }
            drop(ctx);
        });
    // Persist the Edit sub-tab's auto-solve budget the same way -- see
    // `AppSettings::editor_auto_solve_budget_ms`'s own doc comment and
    // `gui::editor::auto_solve`'s module doc comment for the setting's effect.
    let settings_store_auto_solve_budget = settings_store.clone();
    ui.global::<EditorModel>()
        .on_auto_solve_budget_ms_changed(move |budget_ms: i32| {
            settings_store_auto_solve_budget.update(|s| {
                s.settings.editor_auto_solve_budget_ms =
                    u32::try_from(budget_ms).unwrap_or_default();
            });
        });
    library::remote::setup_library_source_callbacks(&ui, &db, &library_source, &settings_store);
    library::remote::setup_mirror_sync_callbacks(&ui, &db, &settings_store);
    setup_worker_callbacks(&ui, &render_ctx, &settings_store);
    // Cached design-catalogue preview thumbnails (front/top renders) -- decoded and
    // cached off the UI thread (see `preview_cache`'s own doc comment), and the
    // background batch job that generates them (two-level progress, cancellation,
    // per-item panic isolation, local/remote dispatch over a shared work queue -- see
    // `preview_batch`'s own doc comment). The batch-callbacks setup also kicks off the
    // once-per-session "designs with no previews yet" scan/offer (trigger: library
    // scan) as part of its own wiring.
    batch::preview_cache::setup_preview_thumbnail_callback(&ui, &db);
    batch::preview::setup_preview_batch_callbacks(&ui, &db, &render_ctx, &settings_store);
    // Tilt-curve batch computation -- same shape as the preview batch above,
    // deliberately never auto-scanned at startup (see `tilt_batch`'s own module doc
    // comment for why: the cost per catalogue is roughly an order of magnitude
    // higher).
    batch::tilt::setup_tilt_batch_callbacks(&ui, &db, &render_ctx, &settings_store);
    // Preview-then-handoff remote rendering: a repeating `slint::Timer` that polls
    // camera/light pose and drives the handoff -- must be kept alive for the life of
    // the window (a dropped `Timer` simply stops firing), hence the binding held all
    // the way to `ui.run()` below rather than being dropped immediately.
    let remote_rendering_timer = setup_remote_rendering(&ui, &render_ctx, &settings_store);

    // Signal the render thread to exit when the window closes. `running` is the thread's
    // shutdown flag -- it's only ever set to false here, once, since the thread has no way to
    // be restarted. Without this the render thread would silently outlive the window.
    //
    // Gated on `EditorModel.is_dirty` so an unsaved Edit-tab design is never
    // discarded by one click on the window's own close button with no prompt.
    // A dirty design keeps the window open and shows `MainWindow.close_confirm_open`'s
    // Save/Discard/Cancel guard instead (see `close_confirm_open`'s own doc comment in
    // `app.slint`); `close_confirm_save`/`close_confirm_discard` below do the actual
    // shutdown once the user picks one.
    let render_ctx_close = render_ctx.clone();
    let settings_store_close = settings_store.clone();
    let ui_weak_close = ui.as_weak();
    ui.window().on_close_requested(move || {
        let is_dirty = ui_weak_close
            .upgrade()
            .is_some_and(|ui| ui.global::<EditorModel>().get_is_dirty());
        if !is_dirty {
            render_ctx_close
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .running = false;
            // Bypasses the debounce window -- a change made in the last `DEBOUNCE`
            // interval before quitting must not be silently lost.
            settings_store_close.flush();
            return slint::CloseRequestResponse::HideWindow;
        }
        if let Some(ui) = ui_weak_close.upgrade() {
            ui.set_close_confirm_open(true);
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    setup_close_confirm_callbacks(&ui, &render_ctx, &settings_store);

    Ok(MainWindowHandle {
        ui,
        render_ctx,
        settings_store,
        remote_rendering_timer,
    })
}

/// `MainWindow.close_confirm_save`/`close_confirm_discard` -- the window-close
/// unsaved-changes guard's two ways past itself (`close_confirm_open`'s own doc
/// comment in `app.slint`; Cancel needs no Rust handler at all, see that same
/// comment). Both finish with the same `render_ctx.running = false` +
/// `settings_store.flush()` + hide sequence that `on_close_requested` runs
/// directly when the design is already clean (not dirty).
///
/// "Save" invokes `EditorModel.save_native` and only proceeds to actually close once
/// that save left the design clean -- a cancelled or failed save (already toasted by
/// `save_native` itself) leaves the window open instead of closing out from under an
/// unsaved design anyway.
fn setup_close_confirm_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx_save = render_ctx.clone();
    let settings_store_save = settings_store.clone();
    let ui_weak_save = ui.as_weak();
    ui.on_close_confirm_save(move || {
        let Some(ui) = ui_weak_save.upgrade() else {
            return;
        };
        ui.set_close_confirm_open(false);
        ui.global::<EditorModel>().invoke_save_native();
        if ui.global::<EditorModel>().get_is_dirty() {
            return;
        }
        render_ctx_save
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .running = false;
        settings_store_save.flush();
        let _ = ui.hide();
    });

    let render_ctx_discard = render_ctx.clone();
    let settings_store_discard = settings_store.clone();
    let ui_weak_discard = ui.as_weak();
    ui.on_close_confirm_discard(move || {
        let Some(ui) = ui_weak_discard.upgrade() else {
            return;
        };
        ui.set_close_confirm_open(false);
        render_ctx_discard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .running = false;
        settings_store_discard.flush();
        let _ = ui.hide();
    });
}

/// Resolves the starting directory for a native `rfd` file/folder picker from a path
/// text field's current value: the field's own value if it already names an existing
/// directory, that path's parent if it names an existing file, otherwise `None`
/// (callers then leave `rfd`'s directory unset). Shared by every picker call site that
/// still fills a text field this way (`camera_lighting`'s HDR environment-map path,
/// `remote::worker_callbacks`'s certificate-bundle-folder field) so "what counts as a
/// path already in the field" stays one rule.
fn starting_dir_from_picker_field(current: &str) -> Option<std::path::PathBuf> {
    if current.is_empty() {
        return None;
    }
    let path = std::path::Path::new(current);
    if path.is_dir() {
        Some(path.to_path_buf())
    } else if path.is_file() {
        path.parent().map(std::path::Path::to_path_buf)
    } else {
        None
    }
}

/// Candidate `docs/manual/README.md` locations for [`locate_user_manual`], relative to
/// a base directory (the running executable's own directory in production; see that
/// function for how each candidate is built from it).
///
/// Order matters: earlier candidates are preferred, and the first one that exists on
/// disk wins. The first three match how the manual is actually laid out relative to
/// the installed/built binary (bundled alongside it, one level up, or in the `cargo
/// build` target-directory layout); the last is a dev-only fallback for running
/// straight out of a `cargo run` checkout, where none of those relative layouts apply.
fn user_manual_candidates(exe_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    vec![
        exe_dir.join("docs/manual/README.md"),
        exe_dir.join("../docs/manual/README.md"),
        // `cargo build` layout: `<target>/<profile>/<exe>` -> the crate root is two
        // directories up from the executable's directory.
        exe_dir.join("../../apps/indicatrix-cut/docs/manual/README.md"),
        // Dev fallback: running via `cargo run` from within this crate's own checkout,
        // where the exe-relative candidates above don't line up with the source tree.
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/manual/README.md"),
    ]
}

/// Resolves the bundled user manual's path at runtime: tries each of
/// [`user_manual_candidates`], relative to the running executable's own directory, in
/// order, and returns the first one that exists on disk. `None` if none of them do
/// (e.g. a build that never bundled the manual at all).
///
/// Runtime-resolved rather than the `CARGO_MANIFEST_DIR` compile-time path this used
/// to be: that path is baked in at compile time and only ever resolves correctly for a
/// build run from within this crate's own checkout, never for a distributed binary.
fn locate_user_manual() -> Option<std::path::PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))?;
    user_manual_candidates(&exe_dir)
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Help menu: opens the bundled `docs/manual/README.md` via the same per-platform
/// `xdg-open`/`cmd /C start`/`open` dispatch
/// `library::diagram_list::setup_diagram_selection_and_export_callbacks`'s
/// `on_open_diagram_url` uses for URLs (all three also open a plain file path). The
/// path itself is resolved at runtime by [`locate_user_manual`] (see its doc comment
/// for the candidate search order). If none of those candidates exist on disk, this
/// shows an error toast instead of silently doing nothing.
fn setup_user_manual_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.on_open_user_manual(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let Some(manual_path) = locate_user_manual() else {
            show_toast(&ui, "User manual not found", "error");
            return;
        };
        open_with_os_handler(&manual_path.to_string_lossy());
    });
}

/// Hands `path` (a file, a folder, or a URL -- all three work on every branch) to
/// the platform's own opener. Failures are deliberately ignored: there is no
/// portable way to tell "no handler registered" from "the handler launched and
/// exited", and every caller here has already put the path on screen, so the
/// cutter is never left with nothing.
fn open_with_os_handler(path: &str) {
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", path])
        .spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
}

/// Reveals whatever the Edit tab last saved or exported, by
/// opening its containing FOLDER (not the file -- opening a `.asc` would launch
/// whatever text editor is registered for it, which is not what "where did it
/// go?" is asking). The path itself is pushed by
/// `gui::editor::native_io::record_last_saved_path`; this is a no-op until the
/// first save of the session, which is also when the strip segment that invokes
/// it first appears.
fn setup_reveal_last_saved_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_reveal_last_saved_path(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let saved = ui.global::<EditorModel>().get_last_saved_path();
            if saved.is_empty() {
                return;
            }
            let path = std::path::PathBuf::from(saved.as_str());
            let target = path.parent().unwrap_or(&path);
            open_with_os_handler(&target.to_string_lossy());
        });
}

/// Shows a toast message.
///
/// Informational/success toasts auto-dismiss after 3.5s; `"error"` and `"warning"`
/// stay on screen until the user dismisses them (via
/// `Toast.dismiss` -> `root.toast_visible = false` in `app.slint`), since a
/// critical outcome the user needed to act on could otherwise disappear before
/// they read it -- see `docs/history/indicatrix-cut.md` for the review this
/// asymmetry came out of. `"warning"` is for a critical-but-not-failed outcome
/// (e.g. "your masts are placeholders", "the saved meet constraints were not
/// restored") that deserves the same persistence as an error without implying the
/// action itself failed; `ui/components/toast.slint`'s `Toast` component has
/// its own amber `"warning"` branch on background/border/icon colour, so a
/// `"warning"` toast renders distinctly from both `"info"` and `"error"` rather
/// than falling back to the info-style default.
///
/// A plain function (not a closure) since it captures nothing from `run_gui` --
/// every `ui.on_X` callback across this module's submodules that needs it just calls
/// it directly on its own `ui: &MainWindow`. `pub` because a downstream binary
/// reusing this window needs to surface its own results through the same toast.
pub fn show_toast(ui: &MainWindow, msg: &str, toast_type: &str) {
    ui.set_toast_message(msg.into());
    ui.set_toast_type(toast_type.into());
    ui.set_toast_visible(true);

    // Every toast bumps this counter; a scheduled dismiss below only acts if it's
    // still THIS call's generation by the time it fires -- otherwise a second toast
    // shown while the first one's timer is still running would get hidden early by
    // that stale timer (see `MainWindow.toast_generation`'s own doc comment).
    let generation = ui.get_toast_generation() + 1;
    ui.set_toast_generation(generation);

    // Errors and warnings stay until the user dismisses them -- see this
    // function's own doc comment.
    if toast_type == "error" || toast_type == "warning" {
        return;
    }

    let ui_weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(3500), move || {
        if let Some(ui) = ui_weak.upgrade()
            && ui.get_toast_generation() == generation
        {
            ui.set_toast_visible(false);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{initial_custom_materials_and_specific_gravity, user_manual_candidates};
    use indicatrix_vault::model::material::CustomMaterialRow;

    fn custom_row(name: &str, specific_gravity: Option<f32>) -> CustomMaterialRow {
        CustomMaterialRow {
            name: name.to_string(),
            refractive_index: 1.7,
            dispersion: 0.02,
            birefringence: 0.0,
            absorption_rgb: [0.0, 0.0, 0.0],
            crystal_system: None,
            optical_character: None,
            biaxial_delta_beta_alpha: None,
            per_axis_dispersion_json: None,
            specific_gravity,
        }
    }

    // --- initial_custom_materials_and_specific_gravity ---

    /// Every row becomes a `GemMaterial` regardless of whether it has a recorded
    /// SG, but only the rows that DO have one contribute an entry to the second
    /// tuple element -- the exact split `RenderContext::custom_materials`/
    /// `custom_material_specific_gravity` themselves keep, now populated at
    /// startup from the same read instead of staying empty until a save/delete
    /// callback has run this session.
    #[test]
    fn startup_helper_populates_both_lists_from_one_row_read() {
        let rows = [
            custom_row("My Garnet", Some(3.90)),
            custom_row("Custom Diamond", None),
        ];
        let (materials, specific_gravity) = initial_custom_materials_and_specific_gravity(&rows);

        assert_eq!(materials.len(), 2, "every row becomes a GemMaterial");
        assert!(materials.iter().any(|m| m.name == "My Garnet"));
        assert!(materials.iter().any(|m| m.name == "Custom Diamond"));

        // Exact `f32` -> `f64` widening (no arithmetic in between), but not
        // literal-equal to a directly-typed `f64` -- see `crystal_optics::
        // custom_material_specific_gravity_from_rows`'s own tests for why this
        // compares by name and by tolerance rather than a brittle `assert_eq!`
        // against a `vec![...]` literal.
        assert_eq!(
            specific_gravity.len(),
            1,
            "only the row with a recorded SG contributes an entry, and a row \
             with none is skipped rather than showing up as a spurious Some(0.0)"
        );
        assert_eq!(specific_gravity[0].0, "My Garnet");
        assert!((specific_gravity[0].1 - 3.90_f64).abs() < 1e-6);
    }

    /// An empty catalogue (fresh install, or a session with no custom materials
    /// saved yet) must not panic and must produce two empty lists, never a
    /// default/placeholder entry.
    #[test]
    fn startup_helper_is_empty_for_no_custom_materials() {
        let (materials, specific_gravity) = initial_custom_materials_and_specific_gravity(&[]);
        assert_eq!(materials.len(), 0);
        assert_eq!(specific_gravity.len(), 0);
    }

    /// The candidate list is built entirely from the given base directory (plus one
    /// fixed dev-only `CARGO_MANIFEST_DIR` fallback) -- no hidden dependence on the
    /// current working directory or environment. Also pins down the order (exe dir,
    /// one level up, two levels up under the `cargo build` layout, then the dev
    /// fallback) since [`locate_user_manual`](super::locate_user_manual) relies on
    /// earlier candidates being preferred.
    #[test]
    fn user_manual_candidates_are_relative_to_exe_dir_in_order() {
        let exe_dir = std::path::Path::new("/opt/indicatrix-cut/bin");
        let candidates = user_manual_candidates(exe_dir);

        assert_eq!(
            candidates[0],
            std::path::Path::new("/opt/indicatrix-cut/bin/docs/manual/README.md")
        );
        assert_eq!(
            candidates[1],
            std::path::Path::new("/opt/indicatrix-cut/bin/../docs/manual/README.md")
        );
        assert_eq!(
            candidates[2],
            std::path::Path::new(
                "/opt/indicatrix-cut/bin/../../apps/indicatrix-cut/docs/manual/README.md"
            )
        );
        // Last candidate is the fixed dev-only fallback, independent of `exe_dir`.
        assert_eq!(
            candidates[3],
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/manual/README.md")
        );
        assert_eq!(candidates.len(), 4);
    }
}
