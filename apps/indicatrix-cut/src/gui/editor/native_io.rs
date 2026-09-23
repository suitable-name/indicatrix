//! File-based import/export for the Edit tab: exporting the edited schedule as a
//! plain `.asc` ([`setup_export_asc_callback`]), and the paired native
//! `.indicatrix.toml` save/open ([`setup_save_native_callback`]/
//! [`setup_open_native_callback`]; legacy `.gemcut.toml` sidecars still open). See
//! this group's own `mod.rs` doc comment.

use super::{
    callbacks::clear_analysis_results,
    state::{EditorState, MaterialComboCache, PendingUnsavedAction, PushedScratch},
    view::refresh_all,
};
use crate::{
    EditorModel, MainWindow,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::{show_toast, solid_preview::preview_state::SolidPreviewState},
};
use indicatrix::{
    geometry::{meet_solver::SolvedTier, stone_metrics::ExternalProportions},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    Design, FingerprintCheck, History, LoadPairedResult, NativeDesignFile, TierOverlay,
    built_in_refractive_index, load_paired, native_path_for_asc,
};
// Custom materials are attached/restored to a native sidecar -- see
// `custom_material_snapshot_for_save` (save side) and
// `restore_custom_material_if_needed` (load side).
//
// `SaveExtras::custom_catalogue`: the field that resolves a CUSTOM material's own
// refractive index for the written `.asc`'s `I` line, rather than the legacy
// schedule RI -- see `snapshot_custom_materials`'s own doc comment for how this
// module threads `RenderContext::custom_materials` into it.
use indicatrix_cut_core::native::{
    CustomMaterialSnapshot, LoadNativeOnlyResult, PairedSave, SaveError, SaveExtras,
    gem_material_from_custom_snapshot, load_native_only, save_native_only_toml,
    save_paired_extended, save_paired_extended_from_solved,
};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tracing::warn;

// This module uses a `SolveService` worker -- see `resolve_solved_then`'s own doc
// comment, below.
use super::solve_service::{SolveKind, SolveOutcome, SolveRequest, SolveService};

// =============================================================================
// Shared infrastructure for write/export/autosave/open paths. Three groups of
// operations each get one shared entry point below:
// 1. Group 1: no inline solves (use cached solve or background solve)
// 2. Group 2: in-window confirmations
// 3. Group 3: file pickers off the UI thread
// =============================================================================

// --- Group 1: a matching cached solve, else a background solve, never inline ---

/// Pure decision half of the cached-solve reuse this group's write paths use in
/// place of a fresh `Design::solve()` on the UI thread: `Some` iff `cached` has one
/// entry per `design.tiers` -- the same alignment contract
/// [`Design::to_asc_schedule_from_solved_with`]/[`Design::planes_from_solved`]
/// document on their own `solved` parameter. Split out from
/// [`cached_solve_matching`] purely so a test can drive it with a plain fixture,
/// with no `auto_solve` runtime involved.
///
/// This is a conservative PROXY for "describes the design as it is right now," not
/// a true generation match: an edit that changes a tier's angle/index without
/// changing the tier COUNT (an ordinary angle nudge, say) leaves a stale-but-
/// same-shaped cached solve looking usable. `auto_solve::solid_last_solved`'s own
/// cache (`super::auto_solve::solid_last_solved`) carries no generation tag of its
/// own to check against (it is set once per completed background solve and never
/// cleared on an edit -- see that function's own doc comment) -- closing this gap
/// would need `SolidLastSolved` itself, or a paired generation counter, to carry
/// one, which is `auto_solve.rs`'s own type, defined outside this module. Every
/// call site below still treats "no usable cache" the same as "cache empty": it
/// submits a real, off-UI-thread solve
/// rather than trusting a possibly-stale one, so the exposure here is narrower than
/// it sounds -- a false "still matches" only survives until the next background
/// solve completes for this same tier count.
#[must_use]
fn solve_matches_design(cached: Option<&[SolvedTier]>, design: &Design) -> Option<Vec<SolvedTier>> {
    let cached = cached?;
    (cached.len() == design.tiers.len()).then(|| cached.to_vec())
}

/// Reads this module's own cached last-completed background solve and returns it
/// only when [`solve_matches_design`] accepts it against `design`. See that
/// function's own doc comment for what "matches" does and does not guarantee.
fn cached_solve_matching(design: &Design) -> Option<Vec<SolvedTier>> {
    let cache = super::auto_solve::solid_last_solved()?;
    let guard = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    solve_matches_design(guard.as_deref(), design)
}

/// A stashed [`resolve_solved_then`] continuation -- named purely to keep
/// [`PENDING_SOLVES`]'s own type under clippy's `type_complexity` lint.
type SolveContinuation = Box<dyn FnOnce(&MainWindow, Result<Vec<SolvedTier>, String>)>;

thread_local! {
    /// This module's own persistent [`SolveService`] worker -- created lazily on
    /// first use ([`ensure_solve_service`]). `setup_editor_callbacks` does not own
    /// one to share with this module, and every write/export/autosave site here
    /// needs the same one worker, so this module owns it directly. One worker for
    /// the life of the window, the same reasoning [`AUTOSAVE_TIMER`] documents on
    /// itself.
    static SOLVE_SERVICE: RefCell<Option<SolveService>> = const { RefCell::new(None) };
    /// Continuations for a request submitted to [`SOLVE_SERVICE`], keyed by the
    /// `generation` `SolveRequest`/`SolveResult` echo back -- reused purely as an
    /// opaque continuation key (see `SolveRequest::generation`'s own doc comment:
    /// "opaque to this module"), since none of this module's callers have a real
    /// domain generation counter to compare a background solve against (that
    /// belongs to `EditorState`, in `state/mod.rs`).
    static PENDING_SOLVES: RefCell<std::collections::HashMap<u64, SolveContinuation>> =
        RefCell::new(std::collections::HashMap::new());
    static NEXT_SOLVE_KEY: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Creates [`SOLVE_SERVICE`]'s worker the first time this module ever needs one.
fn ensure_solve_service(ui: &MainWindow) {
    SOLVE_SERVICE.with(|cell| {
        if cell.borrow().is_some() {
            return;
        }
        let service = SolveService::new(
            ui.as_weak(),
            // No progress UI is wired to this module's own write/export/autosave
            // paths yet -- a future change can render `SolveProgressReport` the
            // same way `deep_solve`'s own status line does.
            |_ui, _progress| {},
            |ui, result| {
                let Some(on_done) =
                    PENDING_SOLVES.with(|c| c.borrow_mut().remove(&result.generation))
                else {
                    // Already delivered, or this module never submitted it.
                    return;
                };
                let outcome = match result.outcome {
                    SolveOutcome::Solved(Ok(solved)) => Ok(solved),
                    SolveOutcome::Solved(Err(e)) => Err(e.to_string()),
                    // This module only ever submits `SolveKind::Full` -- the other
                    // kinds are reserved for other future callers, see
                    // `solve_service.rs`'s own doc comment.
                    SolveOutcome::Verified(_) => Err("unexpected solve result kind".to_string()),
                };
                on_done(ui, outcome);
            },
        );
        *cell.borrow_mut() = Some(service);
    });
}

/// Submits a full solve of `design` to this module's own [`SOLVE_SERVICE`],
/// delivering the result to `on_done` once it lands (never inline on the UI
/// thread's own call stack). Used only when [`cached_solve_matching`] has nothing
/// usable.
fn submit_full_solve(
    ui: &MainWindow,
    design: Arc<Design>,
    on_done: impl FnOnce(&MainWindow, Result<Vec<SolvedTier>, String>) + 'static,
) {
    ensure_solve_service(ui);
    let key = NEXT_SOLVE_KEY.with(|c| {
        let key = c.get();
        c.set(key + 1);
        key
    });
    PENDING_SOLVES.with(|cell| {
        cell.borrow_mut().insert(key, Box::new(on_done));
    });
    SOLVE_SERVICE.with(|cell| {
        if let Some(service) = cell.borrow().as_ref() {
            service.submit(SolveRequest {
                design,
                generation: key,
                kind: SolveKind::Full,
            });
        }
    });
}

/// Group 1's entry point: resolves a design's solve either from cache (synchronous)
/// or background solve (asynchronous). Uses [`cached_solve_matching`] when it has
/// an answer (synchronous, no perceptible cost), else [`submit_full_solve`]
/// (asynchronous -- `then` runs once the background worker reports back). Either
/// way `then` runs exactly once, with `design` handed back alongside the result
/// so a caller need not keep its own separate clone around just to reach it from
/// the continuation.
///
/// Reused by `callbacks::retarget_actions::setup_snapshot_callbacks` for its
/// cached-or-background resolution -- see that function's own doc comment.
pub(super) fn resolve_solved_then(
    ui: &MainWindow,
    design: Arc<Design>,
    then: impl FnOnce(&MainWindow, Arc<Design>, Result<Vec<SolvedTier>, String>) + 'static,
) {
    if let Some(solved) = cached_solve_matching(&design) {
        then(ui, design, Ok(solved));
        return;
    }
    let for_submit = Arc::clone(&design);
    submit_full_solve(ui, for_submit, move |ui, result| {
        then(ui, design, result);
    });
}

// --- Group 2: in-window write confirmations (no more blocking native message dialogs) -

/// [`decide_write_status`]'s outcome -- pure decision from a design's own status,
/// no dialog shown yet.
enum StatusDecision {
    /// `design.status()` names no problem -- proceed without asking.
    Fine,
    /// A real problem exists; the cutter must be asked before writing. Carries the
    /// same message the confirm dialog shows (and, if accepted, the message
    /// [`degenerate_marker_header`] stamps into the written file).
    NeedsConfirm(String),
}

/// Decides whether a design can be written based on its status: takes an
/// already-resolved solve (see [`resolve_solved_then`]) instead of solving `design`
/// itself, upholding the principle that the UI thread never solves.
/// `solved`'s `Err` side is the stringified solve error (`MissingAnchor`'s own
/// `Display`, via `DesignSolveError`'s -- see `resolve_solved_then`'s own doc
/// comment).
#[must_use]
fn decide_write_status(design: &Design, solved: Result<&[SolvedTier], &str>) -> StatusDecision {
    match solved {
        Ok(solved) => {
            let (message, is_problem) =
                super::state::status_text_and_is_problem_from_solved(design, solved);
            if is_problem {
                StatusDecision::NeedsConfirm(message)
            } else {
                StatusDecision::Fine
            }
        }
        Err(missing) => StatusDecision::NeedsConfirm(missing.to_string()),
    }
}

/// [`ask_write_confirm`]'s stashed continuation.
struct PendingWriteConfirm {
    on_accept: Box<dyn FnOnce(&MainWindow)>,
    /// The [`AppSettings::suppressed_confirmations`] key this prompt offers
    /// "Don't ask again" under, if any -- read (alongside
    /// `EditorModel.write_confirm_dont_ask`'s own live checkbox state) by
    /// `setup_write_confirm_dialog_callbacks`'s accept handler, to decide whether
    /// to persist the suppression before running [`Self::on_accept`].
    suppress_key: Option<&'static str>,
}

/// Stable [`AppSettings::suppressed_confirmations`] keys for the two suppressible
/// write-confirm prompts. The unsaved-changes/fingerprint-mismatch/gear-remap guards
/// use their own separate `ConfirmActionDialog` mounts (`app.slint`) and are
/// deliberately NOT wired to `show_dont_ask` at all -- a wrong "don't ask again"
/// there risks real data loss, unlike these two (which only ever affect whether a
/// header note gets written).
pub(in crate::gui) mod confirm_keys {
    /// [`super::finish_native_save`]/the Export `.asc` path's "this design is not
    /// a closed solid" prompt.
    pub(in crate::gui) const NOT_CLOSED_SOLID: &str = "write_confirm.not_closed_solid";
    /// [`super::confirm_overwrite_unrelated_native_file_then`]'s "overwrite a
    /// native file that belongs to a different design" prompt.
    pub(in crate::gui) const OVERWRITE_UNRELATED_NATIVE: &str =
        "write_confirm.overwrite_unrelated_native";
}

/// Reads whether the confirm prompt named `key` is currently suppressed -- a
/// direct settings-file read (not the debounced `SettingsPersister`, which this
/// module has no handle to; same precedent [`record_recent_native_file`]
/// documents on itself for the identical constraint).
fn confirm_is_suppressed(key: &str) -> bool {
    let settings_path = crate::settings::store::default_settings_path();
    crate::settings::store::load_or_default(&settings_path)
        .settings
        .is_confirm_suppressed(key)
}

/// Persists that the confirm prompt named `key` should not be shown again.
///
/// `let _ =` on the write: a failure here (a read-only settings directory, say)
/// only means the NEXT save/export asks again -- never that this save/export
/// itself failed or that any design data was lost, and `record_recent_native_file`
/// already accepts the identical direct-write race/failure mode on this exact
/// settings file for the same reason (no debounced-persister handle in this
/// module).
fn suppress_confirm_permanently(key: &'static str) {
    let settings_path = crate::settings::store::default_settings_path();
    let mut file = crate::settings::store::load_or_default(&settings_path);
    file.settings.suppress_confirm(key);
    let _ = crate::settings::store::save(&settings_path, &file);
}

thread_local! {
    /// The continuation [`ask_write_confirm`] stashed, if the write-confirm dialog
    /// is currently open -- `None` whenever it is closed (the common state), the
    /// same shape [`PENDING_MISMATCH`] uses for its own dialog.
    static PENDING_WRITE_CONFIRM: RefCell<Option<PendingWriteConfirm>> = const { RefCell::new(None) };
}

/// Opens the shared in-window write-confirm dialog (`EditorModel.write_confirm_*`,
/// mounted in `ui/app.slint`) and stashes `on_accept` to run once the cutter
/// accepts. A decline (Cancel) drops it with no toast.
///
/// Reused by `gui::library::local::import`'s "replace existing design(s)?" prompt
/// (see that module's own `confirm_collisions_then`). `EditorModel.write_confirm_*`
/// is a plain global with no notion of which caller is asking, so there is nothing
/// import-specific this dialog needs to know; only this function (and the
/// `PENDING_WRITE_CONFIRM` continuation it stashes) is reachable from outside this
/// module.
///
/// `suppress_key`: `Some` offers "Don't ask again" (`ConfirmActionDialog.
/// show_dont_ask`), persisting the choice via [`suppress_confirm_permanently`] the
/// moment the cutter accepts WITH the checkbox ticked -- and, before even opening
/// the dialog, skips it entirely (runs `on_accept` immediately) if this same key
/// was already suppressed on an earlier run. `None` keeps asking every time,
/// with no checkbox shown at all.
pub(in crate::gui) fn ask_write_confirm(
    ui: &MainWindow,
    heading: &str,
    message: String,
    primary_label: &str,
    suppress_key: Option<&'static str>,
    on_accept: impl FnOnce(&MainWindow) + 'static,
) {
    if suppress_key.is_some_and(confirm_is_suppressed) {
        on_accept(ui);
        return;
    }
    PENDING_WRITE_CONFIRM.with(|cell| {
        *cell.borrow_mut() = Some(PendingWriteConfirm {
            on_accept: Box::new(on_accept),
            suppress_key,
        });
    });
    let model = ui.global::<EditorModel>();
    model.set_write_confirm_heading(heading.into());
    model.set_write_confirm_message(message.into());
    model.set_write_confirm_primary_label(primary_label.into());
    model.set_write_confirm_show_dont_ask(suppress_key.is_some());
    model.set_write_confirm_dont_ask(false);
    model.set_write_confirm_open(true);
}

/// Registers the write-confirm dialog's own two callbacks -- bundled into
/// [`setup_save_native_callback`] (called exactly once, like every other `setup_*`
/// entry point here) rather than given its own, the same reasoning
/// [`setup_dirty_tracking`]/[`setup_autosave_timer`] document on themselves.
fn setup_write_confirm_dialog_callbacks(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_write_confirm_accept(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let model = ui.global::<EditorModel>();
        model.set_write_confirm_open(false);
        let Some(pending) = PENDING_WRITE_CONFIRM.with(RefCell::take) else {
            return;
        };
        // Persist the suppression BEFORE running `on_accept` -- that closure may
        // itself trigger a save that reads this same settings file back (e.g.
        // `record_recent_native_file`), so the write must land first.
        if let Some(key) = pending.suppress_key
            && model.get_write_confirm_dont_ask()
        {
            suppress_confirm_permanently(key);
        }
        super::stall_guard::stall_guard("write_confirm_accept", || (pending.on_accept)(&ui));
    });
    ui.global::<EditorModel>().on_write_confirm_cancel(move || {
        // The cutter's explicit cancel -- no toast, nothing runs.
        PENDING_WRITE_CONFIRM.with(|cell| *cell.borrow_mut() = None);
    });
}

// --- Group 3: file pickers off the UI thread --------------------------------
//
// The picker machinery itself (a `PickKind`-flavoured native file-dialog
// builder, the UI-thread rendezvous, and the `#[cfg(test)]` answer-injection
// hook) lives in `gui::pickers` -- the one place the `rfd` crate's file dialog
// is ever constructed anywhere in this app, so every OTHER module's own
// picker uses the SAME worker instead of each spawning its own thread and
// rendezvous. `PickKind`/`pick_file` here are a thin translation over
// `gui::pickers::{PickerKind, PickerRequest, pick}` -- every call site below
// builds a `PickKind` value and threads a continuation through it. See
// `gui::pickers`'s own module doc comment for why a request enum/struct, not a
// caller-supplied native-file-dialog builder closure.

/// Which native picker [`pick_file`] should show -- one entry per save/open
/// dialog this module's own write/export/open paths need, translated to a
/// [`super::super::pickers::PickerRequest`] by [`pick_file`] itself.
enum PickKind {
    /// "Export .asc" / "Save Native As...": the `.asc` save-as picker.
    SaveAsc { default_name: String },
    /// "Export Cutting Sheet": the `.html` save-as picker.
    SaveCuttingSheet { default_name: String },
    /// "Export Diagram": the `.png` save-as picker.
    SaveDiagram { default_name: String },
    /// "Open Native": accepts a native sidecar or a bare `.asc`.
    OpenNativeOrAsc,
    /// [`resolve_paired_asc_text_then`]'s "Locate the paired .asc" recovery picker.
    LocateAsc,
}

/// The `.asc design` filter every save/open dialog below that touches a
/// `.asc` file shares -- a `fn`, not a `const`, since [`PickerFilter`] now
/// owns its data (`String`/`Vec<String>`, see that type's own doc comment for
/// why), which cannot be built in a `const` context.
fn asc_filter() -> crate::gui::pickers::PickerFilter {
    crate::gui::pickers::PickerFilter {
        label: ".asc design".to_string(),
        extensions: vec!["asc".to_string()],
    }
}

/// Translates `kind` into a [`super::super::pickers::PickerRequest`] and shows
/// it via [`super::super::pickers::pick`] -- the one entry point every native
/// file picker in this module uses. `state` must never be borrowed across a
/// call to this function -- every call site below picks first, then borrows,
/// never the other way around.
fn pick_file(
    ui: &MainWindow,
    kind: PickKind,
    on_done: impl FnOnce(&MainWindow, Option<PathBuf>) + 'static,
) {
    use crate::gui::pickers::{PickerFilter, PickerKind, PickerRequest, pick};

    let request = match kind {
        PickKind::SaveAsc { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![asc_filter()],
            default_file_name: Some(default_name),
            // Seeds `./exports` as the starting directory when it exists, the same
            // convention `gui::library::local::export::setup_export_asc_callback`/
            // `gui::library::detail::export_diagram_file_via_source` already
            // use -- a no-op (leaves the OS's own last-used-directory memory
            // in place) when that folder doesn't exist yet.
            starting_dir: default_export_dir(),
        },
        PickKind::SaveCuttingSheet { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![PickerFilter {
                label: "Cutting sheet (HTML)".to_string(),
                extensions: vec!["html".to_string()],
            }],
            default_file_name: Some(default_name),
            starting_dir: None,
        },
        PickKind::SaveDiagram { default_name } => PickerRequest {
            kind: PickerKind::SaveFile,
            title: None,
            filters: vec![PickerFilter {
                label: "Diagram (PNG)".to_string(),
                extensions: vec!["png".to_string()],
            }],
            default_file_name: Some(default_name),
            starting_dir: None,
        },
        // The leading filter must use the FULL compound suffix (not
        // a bare `"toml"`) or `rfd` would offer to select `Cargo.toml`.
        PickKind::OpenNativeOrAsc => PickerRequest {
            kind: PickerKind::OpenFile,
            title: Some("Open Native Design".to_string()),
            filters: vec![
                PickerFilter {
                    label: "Indicatrix native design".to_string(),
                    extensions: vec![
                        indicatrix_cut_core::native::NATIVE_EXTENSION_SUFFIX.to_string(),
                        indicatrix_cut_core::native::LEGACY_NATIVE_EXTENSION_SUFFIX.to_string(),
                    ],
                },
                PickerFilter {
                    label: "All TOML files".to_string(),
                    extensions: vec!["toml".to_string()],
                },
                asc_filter(),
            ],
            default_file_name: None,
            starting_dir: None,
        },
        PickKind::LocateAsc => PickerRequest {
            kind: PickerKind::OpenFile,
            title: Some("Locate the paired .asc".to_string()),
            filters: vec![asc_filter()],
            default_file_name: None,
            starting_dir: None,
        },
    };
    pick(ui, request, on_done);
}

/// The file name an Export/Save Native dialog should default to. Prefers
/// `asc_filename` (this design's own recorded/last-saved name -- see
/// [`super::state::EditorState::asc_filename`]'s own doc comment) when set, else a
/// sanitized version of the schedule's own first free-text header line (the `GemCad`
/// convention for a design's title/description), else `"edited_design.asc"` for a
/// design with neither (a brand-new "New Design" with no header typed yet). Shared by
/// [`setup_export_asc_callback`] and [`setup_save_native_callback`] so the two
/// dialogs stay in sync instead of each proposing its own default name.
fn suggested_file_name(st: &EditorState) -> String {
    if let Some(name) = &st.asc_filename {
        return name.clone();
    }
    match st.design.meta.headers.first() {
        Some(header) if !header.trim().is_empty() => {
            format!(
                "{}.asc",
                crate::gui::library::local::sanitize_filename(header)
            )
        }
        _ => "edited_design.asc".to_string(),
    }
}

/// `./exports` as a [`PickKind::SaveAsc`] picker's starting directory, when
/// that folder exists -- the same convention
/// `gui::library::local::export::setup_export_asc_callback`/
/// `gui::library::detail::export_diagram_file_via_source` use (and the one
/// `docs/manual/11-saving-and-file-formats.md` promises). `None` when the
/// folder doesn't exist, so the OS's own last-used-directory memory still
/// applies for a cutter who has never created one -- see
/// [`super::super::pickers::PickerRequest::starting_dir`]'s own doc comment
/// for what a `None` there means.
fn default_export_dir() -> Option<PathBuf> {
    let default_dir = Path::new("exports");
    default_dir.is_dir().then(|| default_dir.to_path_buf())
}

/// This design's currently registered custom catalogue materials -- the same list
/// [`super::view::refresh_design_settings`]'s on-screen "Eff. RI" resolves against
/// (`RenderContext::custom_materials`) -- snapshotted (a cheap `Arc` clone, not a
/// deep copy) for a save/export callback to pass to a `_with` entry point
/// (`Design::to_asc_schedule_with`/`Design::cutting_sheet_with`) or into
/// `SaveExtras::custom_catalogue` (for `save_paired_extended`/
/// `save_paired_extended_from_solved`) so a design on a CUSTOM catalogue material
/// writes/prints that material's own refractive index instead of silently falling
/// back to the legacy schedule RI.
fn snapshot_custom_materials(render_ctx: &Arc<Mutex<RenderContext>>) -> Arc<Vec<GemMaterial>> {
    Arc::clone(
        &render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .custom_materials,
    )
}

/// The leading header a confirmed-with-reason write stamps into a written
/// schedule, so the file itself carries a hint of why it needed confirming. A
/// distinct marker from [`indicatrix_formats::asc::mark_reconstructed`]'s own
/// "RECONSTRUCTED" line: that one is specific to an angle-table placeholder
/// reconstruction, a different situation from a design that solves but does
/// not close a real stone.
const NOT_CLOSED_SOLID_MARKER: &str = "NOT A CLOSED SOLID";

/// Builds the header line itself, or `None` when `headers` already starts with one
/// (never stamped twice, mirroring `mark_reconstructed`'s own idempotence).
fn degenerate_marker_header(headers: &[String], message: &str) -> Option<String> {
    let already_marked = headers
        .first()
        .is_some_and(|h| h.starts_with(NOT_CLOSED_SOLID_MARKER));
    (!already_marked).then(|| format!("{NOT_CLOSED_SOLID_MARKER} -- {message}"))
}

/// A design that solves but does not close a real stone (every mast `0.0`
/// after an angle-table placeholder load is the common way this happens, but a
/// half-built schedule mid-session hits it too) would otherwise export/save as a
/// normal-looking file with no signal beyond a validation banner the cutter may have
/// scrolled past. Checking `design.status()` catches this -- reusing the Edit tab's
/// own validation-banner wording so this asks about exactly the same problem the
/// cutter already saw on screen, never a second, differently-worded description of it.
///
/// Every write call site in this module resolves the design's solve off the UI
/// thread first (via [`resolve_solved_then`]), then decides synchronously with
/// [`decide_write_status`] (a pure decision over the already-resolved solve)
/// whether to prompt with [`ask_write_confirm`] (the in-window dialog, never a
/// blocking native message dialog): `resolve_solved_then` -> `decide_write_status`
/// -> (`Fine`: write immediately) or (`NeedsConfirm`: `ask_write_confirm`, write
/// from its `on_accept`).
///
/// Also returns the `solved` masts themselves (`None` only for a `MissingAnchor`),
/// so a caller that goes on to write the file can pass them straight to
/// [`save_paired_extended_from_solved`] instead of re-solving the same design
/// a second time via `Design::to_asc_schedule` (see [`save_paired_reusing_solve`]).
/// Calls [`save_paired_extended_from_solved`] when `solved` is available and
/// `design` actually has tiers to build masts for, so a caller that already solved
/// once (e.g. [`confirm_status_before_write`], or [`run_autosave_tick`]'s own local
/// solve) never pays for [`save_paired_extended`]'s internal `Design::solve()`
/// a second time. Falls back to [`save_paired_extended`] itself for the two
/// cases `solved` cannot stand in for: `solved` is `None` (the design does not
/// currently solve -- re-solving there is cheap, since `Design::solve` returns
/// `MissingAnchor` before ever running the expensive `solve_meet_points` pass) or
/// `design.tiers` is empty (the tier-less draft path, which never solves at all).
///
/// Every caller populates `extras.custom_catalogue` with its own
/// `RenderContext::custom_materials` snapshot (see `snapshot_custom_materials`), so
/// a design on a CUSTOM catalogue material writes that material's own refractive
/// index to the `.asc` `I` line instead of the legacy schedule RI -- see
/// [`SaveExtras::custom_catalogue`]'s own doc comment.
fn save_paired_reusing_solve(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    asc_filename: impl Into<String>,
    original_asc_text: Option<&str>,
    placeholder_note: Option<&str>,
    printed_proportions: Option<&ExternalProportions>,
    extras: &SaveExtras<'_>,
) -> Result<PairedSave, SaveError> {
    match solved {
        Some(solved) if !design.tiers.is_empty() => save_paired_extended_from_solved(
            design,
            solved,
            asc_filename,
            original_asc_text,
            placeholder_note,
            printed_proportions,
            extras,
        ),
        _ => save_paired_extended(
            design,
            asc_filename,
            original_asc_text,
            placeholder_note,
            printed_proportions,
            extras,
        ),
    }
}

/// Stamps (or clears) `footnotes`' own recorded source-catalogue-row marker before a
/// `.asc` is written, so provenance comes from a recorded id, never a
/// title/filename guess. Removes any previous stamp first (idempotent, same
/// reasoning as [`degenerate_marker_header`]) so a design that changes source row
/// (Save Native creating its very first row) or loses one (the row was deleted) never
/// carries two stamps, or a stale one, across a later save. `gui::library::local::
/// import::save_imported_design` is the reader: a `.asc` re-imported later recovers
/// `source_entry_id` from exactly this line via
/// [`indicatrix_vault::local::parse_source_entry_footnote`] and records it as
/// `diagram_entries.derived_from_entry_id`, turning what would otherwise be a second
/// same-titled row into a recorded version of the original.
///
/// Called from both "Export .asc" and "Save Native": a bare Export with no native
/// sidecar at all still needs to survive an export-then-reimport round trip, so the
/// plain `.asc` itself has to carry this, not only the native sidecar (which already
/// records everything else about this design, but is never attached to a plain
/// Export).
fn stamp_source_entry_footnote(footnotes: &mut Vec<String>, source_entry_id: Option<i64>) {
    footnotes.retain(|f| !f.starts_with(indicatrix_vault::local::SOURCE_ENTRY_FOOTNOTE_PREFIX));
    if let Some(id) = source_entry_id {
        footnotes.push(indicatrix_vault::local::format_source_entry_footnote(id));
    }
}

/// Builds `design`'s [`CustomMaterialSnapshot`] for a native save, so a design saved
/// under a custom material does not silently reload as Diamond -- without this,
/// nothing would attach that material's own numbers to the sidecar. `None` when
/// `design.material.name` is unset, or names one of the
/// built-in presets ([`built_in_refractive_index`] resolves it -- nothing to
/// preserve; any build already knows that name), or names a custom material this
/// build's own database has no row for (nothing to snapshot from).
///
/// Reads the custom-materials DATABASE row rather than a resolved `GemMaterial`:
/// the row holds the cutter's own originally typed mean RI/dispersion/
/// birefringence/specific-gravity numbers directly, while re-deriving them from a
/// resolved `GemMaterial`'s dispersion curve would round-trip through
/// `GemMaterial::new_custom`'s own Cauchy fit for no reason when the authored
/// numbers already exist. `crystal_system`/`optical_character` are carried
/// through as the row's own plain-text names (`""` when the row predates those
/// fields) -- [`gem_material_from_custom_snapshot`] re-derives both from the
/// snapshot's `birefringence_delta` sign on load regardless, so these are for a
/// human reading the raw TOML only.
fn custom_material_snapshot_for_save(
    design: &Design,
    db: &Arc<Mutex<Database>>,
) -> Option<CustomMaterialSnapshot> {
    let name = design.material.name.as_deref()?;
    if built_in_refractive_index(name).is_some() {
        return None;
    }
    let row = db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_custom_materials()
        .ok()?
        .into_iter()
        .find(|r| r.name.eq_ignore_ascii_case(name))?;
    Some(CustomMaterialSnapshot::new(
        f64::from(row.refractive_index),
        f64::from(row.dispersion),
        f64::from(row.birefringence),
        row.specific_gravity.map(f64::from),
        row.crystal_system.unwrap_or_default(),
        row.optical_character.unwrap_or_default(),
    ))
}

/// [`write_back_to_catalogue`]'s outcome, for [`finish_save_native_success`]'s own
/// toast wording -- the ordinary "updated existing row" case gets no extra toast at
/// all (the file-save toast already said enough), while the other two are surprising
/// enough on their own to call out.
enum CatalogueWriteBack {
    /// The design's own known `source_entry_id` still named a real row -- updated in
    /// place. `stale_cache_invalidation_failed` is true when
    /// [`write_back_locked`]'s own `delete_preview_images`/`delete_tilt_curves`
    /// calls could not clear that row's now-outdated cached previews/tilt curves,
    /// so the cutter is told on screen instead of this only being logged via
    /// `tracing::warn!`, where the cache silently showing stale geometry could go
    /// unnoticed.
    UpdatedExisting {
        stale_cache_invalidation_failed: bool,
    },
    /// This design had no known source row -- a brand-new one was inserted.
    NewRow,
    /// This design's known `source_entry_id` no longer named a real row (deleted
    /// while the design was open) -- a brand-new row was inserted instead of an
    /// update, same as [`Self::NewRow`], but worth telling the cutter about.
    SourceRowGoneNewRowCreated,
}

/// This design's catalogue write-back -- called by
/// [`finish_save_native_success`] once [`write_pair_atomically`] has already
/// succeeded (never before: the files on disk are the design of record, so a
/// catalogue-write failure here must never be read as "the save failed").
///
/// Builds the exact same [`indicatrix_vault::local::ImportedAsc`] an Import of these
/// same two just-written files would build
/// ([`indicatrix_vault::local::import_asc`]) plus the same measured proportions/shape
/// an import derives ([`crate::gui::library::local::apply_measured_metadata`]) --
/// this is deliberately the SAME parse-and-measure path, not a second, independently
/// maintained one, so a design's catalogue row always describes it exactly as
/// re-importing the same two files would.
///
/// # What gets overwritten versus preserved
///
/// - `source_entry_id: Some(id)`, `id` still a real row: `angle_settings_table`/
///   `attached_files` (the `.asc` + native sidecar) and every geometry-derived
///   column (`refractive_index`/`index_gear`/`facets_count`/`symmetry_order`/
///   `mirror_symmetry`/the measured `lw`/`hw`/`cw`/`pw`/`volume` ratios/`shape`) are
///   always replaced with this save's own fresh values. Everything else --
///   `designer_info`, a hand-corrected `shape` override, the competition/citation/
///   scrape-only columns -- is merged forward from the existing row first
///   ([`crate::gui::library::local::merge_reimport_metadata`], the SAME rule
///   already applied to a `.asc` re-import), so a cutter's hand-typed
///   metadata survives a Save exactly as it survives an Import. `title` is left
///   untouched entirely (`Database::update_diagram_entry_url` never touches it --
///   same precedent as `Database::update_diagram_metadata`): a title is something a
///   cutter hand-corrects, never something a geometry write-back should silently
///   rename. Previews and tilt curves are invalidated (deleted, to be regenerated on
///   demand) -- they describe the geometry as it was before this save, same as a
///   `.asc` re-import collision already does.
/// - `source_entry_id: Some(id)`, but `id` no longer names a real row (deleted while
///   this design was open): falls through to the next case, exactly as if
///   `source_entry_id` had been `None`, so this save still lands somewhere instead
///   of silently failing or resurrecting a deleted row.
/// - `source_entry_id: None`: this design has never been saved to the catalogue --
///   inserts a brand-new row (`Database::save_diagram_entry` + `save_diagram_detail`,
///   no merge: there is nothing existing to preserve).
///
/// Returns the row's id on success, so the caller can write it back into
/// [`EditorState::source_entry_id`] -- every save after the FIRST one for a
/// previously row-less design must update that SAME new row, never insert a second.
///
/// # Errors
///
/// A ready-to-toast message. A failure here never rolls back the files
/// [`write_pair_atomically`] already wrote -- the design is safely on disk either
/// way; this only affects whether the library list reflects it yet.
fn write_back_to_catalogue(
    db: &Arc<Mutex<Database>>,
    source_entry_id: Option<i64>,
    asc_filename: &str,
    asc_text: &str,
    native_filename: &str,
    native_toml: &str,
) -> Result<(i64, CatalogueWriteBack), String> {
    let mut parsed = indicatrix_vault::local::import_asc(
        asc_filename,
        asc_text,
        Some((native_filename, native_toml.as_bytes())),
    )
    .map_err(|e| format!("Saved to disk, but could not update the catalogue: {e}"))?;
    crate::gui::library::local::apply_measured_metadata(&mut parsed.detail);

    // The lock covers the database work and nothing else: the caller goes on to
    // toast and refresh the library list, and holding the catalogue mutex across
    // that would serialise it against every other reader.
    let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    write_back_locked(&db, source_entry_id, &mut parsed)
}

/// Deletes `existing_id`'s cached preview images and tilt curves (both now stale:
/// the design they describe was just overwritten), logging -- not propagating -- a
/// failure on either, and reporting whether either one failed so the caller can
/// surface it as `CatalogueWriteBack::UpdatedExisting::
/// stale_cache_invalidation_failed`. Two independent expressions rather than a
/// mutable accumulator reassigned by two `if let Err` statements: each deletion's
/// own outcome is total (`Result::is_err`), so there is nothing conditional left to
/// express imperatively.
fn invalidate_stale_caches(db: &Database, existing_id: i64) -> bool {
    let preview_failed = db
        .delete_preview_images(existing_id)
        .inspect_err(|e| {
            warn!(
                "Save Native: failed to invalidate stale preview cache for entry \
                 #{existing_id}: {e}"
            );
        })
        .is_err();
    let tilt_curves_failed = db
        .delete_tilt_curves(existing_id)
        .inspect_err(|e| {
            warn!(
                "Save Native: failed to invalidate stale tilt-curve cache for entry \
                 #{existing_id}: {e}"
            );
        })
        .is_err();
    preview_failed || tilt_curves_failed
}

/// [`write_back_to_catalogue`]'s database half, with the lock already taken --
/// split out so the guard's scope is exactly this call rather than the remainder of
/// its caller.
///
/// Returns the row id written and which of the three cases happened: the design's
/// own row updated in place, its row found missing and a fresh one inserted, or a
/// first-ever insert for a design that had no row.
fn write_back_locked(
    db: &Database,
    source_entry_id: Option<i64>,
    parsed: &mut indicatrix_vault::local::ImportedAsc,
) -> Result<(i64, CatalogueWriteBack), String> {
    if let Some(existing_id) = source_entry_id {
        match db.get_diagram_full(existing_id) {
            Ok(Some(existing)) => {
                crate::gui::library::local::merge_reimport_metadata(&mut parsed.detail, &existing);
                db.update_diagram_entry_url(existing_id, &parsed.entry.url)
                    .map_err(|e| e.to_string())?;
                db.save_diagram_detail(&parsed.detail, existing_id)
                    .map_err(|e| e.to_string())?;
                let stale_cache_invalidation_failed = invalidate_stale_caches(db, existing_id);
                return Ok((
                    existing_id,
                    CatalogueWriteBack::UpdatedExisting {
                        stale_cache_invalidation_failed,
                    },
                ));
            }
            Ok(None) => {
                // Source row deleted while this design was open -- fall through to
                // insert a fresh row below, rather than updating nothing or erroring.
            }
            Err(e) => {
                return Err(format!(
                    "Saved to disk, but could not read this design's catalogue row to \
                     update it: {e}"
                ));
            }
        }
    }

    let new_id = db
        .save_diagram_entry(&parsed.entry, indicatrix_vault::local::LOCAL_SOURCE_ID)
        .map_err(|e| e.to_string())?;
    db.save_diagram_detail(&parsed.detail, new_id)
        .map_err(|e| e.to_string())?;
    let outcome = if source_entry_id.is_some() {
        CatalogueWriteBack::SourceRowGoneNewRowCreated
    } else {
        CatalogueWriteBack::NewRow
    };
    Ok((new_id, outcome))
}

/// "Export Cutting Sheet": the printable HTML sheet.
///
/// Goes through [`resolve_solved_then`] (a matching cached solve, else a
/// background [`SolveService`] solve -- a design that does not solve has no
/// masts to print) and [`pick_file`] (the save-as picker on a background
/// thread), never solving inline on the UI thread or blocking on a native
/// save-file dialog; the HTML build and file write also run on a background
/// thread.
pub(super) fn setup_export_cutting_sheet_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_export_cutting_sheet(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        super::stall_guard::stall_guard("export_cutting_sheet", || {
            let (design, base_name) = {
                let st = state.borrow();
                (st.design.clone(), suggested_file_name(&st))
            };
            let render_ctx = Arc::clone(&render_ctx);
            resolve_solved_then(&ui, Arc::new(design), move |ui, design, solved_result| {
                super::stall_guard::stall_guard("export_cutting_sheet_solved", || {
                    let solved = match solved_result {
                        Ok(solved) => solved,
                        Err(missing) => {
                            show_toast(
                                ui,
                                &format!("Cannot build a cutting sheet: {missing}."),
                                "error",
                            );
                            return;
                        }
                    };
                    // Custom-catalogue-aware: the printed "Refractive index" row
                    // must match a CUSTOM material's own `n_D`.
                    let custom_materials = snapshot_custom_materials(&render_ctx);
                    let base = base_name.trim_end_matches(".asc").to_string();
                    pick_file(
                        ui,
                        PickKind::SaveCuttingSheet {
                            default_name: format!("{base}_cutting_sheet.html"),
                        },
                        move |ui, dest_path| {
                            // A dismissed dialog is the cutter's own cancel -- no
                            // toast.
                            let Some(dest_path) = dest_path else {
                                return;
                            };
                            let ui_weak = ui.as_weak();
                            std::thread::spawn(move || {
                                let result = super::cut_sheet::write_cutting_sheet_html(
                                    &design,
                                    &solved,
                                    &dest_path,
                                    &custom_materials,
                                );
                                let _ = ui_weak.upgrade_in_event_loop(move |ui| match result {
                                    Ok(()) => show_toast(
                                        &ui,
                                        &format!("Wrote cutting sheet to {}", dest_path.display()),
                                        "success",
                                    ),
                                    Err(message) => show_toast(&ui, &message, "error"),
                                });
                            });
                        },
                    );
                });
            });
        });
    });
}

/// "Export Diagram": the 2D crown/pavilion/profile drawing as a PNG -- the same
/// render the cutting sheet embeds, written on its own.
///
/// Follows the same [`resolve_solved_then`] + [`pick_file`] pattern as
/// [`setup_export_cutting_sheet_callback`], never solving inline on the UI
/// thread or blocking on a native file dialog.
pub(super) fn setup_export_diagram_callback(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_export_diagram(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        super::stall_guard::stall_guard("export_diagram", || {
            let (design, base_name) = {
                let st = state.borrow();
                (st.design.clone(), suggested_file_name(&st))
            };
            resolve_solved_then(&ui, Arc::new(design), move |ui, design, solved_result| {
                super::stall_guard::stall_guard("export_diagram_solved", || {
                    let solved = match solved_result {
                        Ok(solved) => solved,
                        Err(missing) => {
                            show_toast(
                                ui,
                                &format!("Cannot draw this design: {missing}."),
                                "error",
                            );
                            return;
                        }
                    };
                    let base = base_name.trim_end_matches(".asc").to_string();
                    pick_file(
                        ui,
                        PickKind::SaveDiagram {
                            default_name: format!("{base}_diagram.png"),
                        },
                        move |ui, dest_path| {
                            let Some(dest_path) = dest_path else {
                                return;
                            };
                            let ui_weak = ui.as_weak();
                            std::thread::spawn(move || {
                                let result = super::cut_sheet::write_diagram_png(
                                    &design, &solved, &dest_path,
                                );
                                let _ = ui_weak.upgrade_in_event_loop(move |ui| match result {
                                    Ok(()) => show_toast(
                                        &ui,
                                        &format!("Wrote diagram to {}", dest_path.display()),
                                        "success",
                                    ),
                                    Err(message) => show_toast(&ui, &message, "error"),
                                });
                            });
                        },
                    );
                });
            });
        });
    });
}

/// "Export .asc": writes the edited schedule (never the catalogue's own, unedited
/// one -- see this group's `mod.rs` doc comment) to a user-chosen path via
/// `indicatrix_formats::to_asc_string`. File only, no database write of any kind --
/// the catalogue stays read-only on this path.
///
/// The "not a closed solid" confirmation goes through [`ask_write_confirm`]
/// (in-window, never blocking) instead of a native (`rfd` crate) message dialog,
/// the save-as picker goes through [`pick_file`] (background thread), and the
/// final `std::fs::write` runs on a background thread. Building `schedule` itself
/// (`to_asc_schedule_with`, which solves internally) stays synchronous on the UI
/// thread, unlike the other write paths in this module
/// ([`confirm_status_before_write`], export cutting sheet, export diagram, the
/// autosave tick), which resolve their solve off-thread via
/// [`resolve_solved_then`]: doing the same here would need either a new
/// `SolveService` request kind that also builds an `AscSchedule` off-thread, or
/// duplicating `Design::to_asc_schedule_with`'s own schedule-building logic in
/// this crate.
pub(super) fn setup_export_asc_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_export_asc(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        super::stall_guard::stall_guard("export_asc", || {
            // Custom-catalogue-aware: a design on a CUSTOM catalogue material must
            // export that material's own `n_D`, not the legacy schedule RI
            // `to_asc_schedule` alone would fall back to.
            let custom_materials = snapshot_custom_materials(&render_ctx);
            // `to_asc_schedule_with` is fallible (it solves every tier's mast --
            // see `Design::to_asc_schedule`'s doc comment): a `MissingAnchor` here
            // means the same "add a Scale Reference tier" problem the validation
            // banner already reports, so it is surfaced the same way (a toast)
            // rather than exporting a schedule with fabricated masts.
            let (mut schedule, default_file_name, design, used_placeholder) = {
                let st = state.borrow();
                let mut schedule = match st.design.to_asc_schedule_with(&custom_materials) {
                    Ok(schedule) => schedule,
                    Err(missing) => {
                        show_toast(&ui, &format!("Cannot export: {missing}."), "error");
                        return;
                    }
                };
                // Recorded so a LATER re-import of this exact file can match it
                // back to its source row instead of creating a second,
                // same-titled duplicate -- see `stamp_source_entry_footnote`'s
                // own doc comment.
                stamp_source_entry_footnote(&mut schedule.footnotes, st.source_entry_id);
                (
                    schedule,
                    suggested_file_name(&st),
                    st.design.clone(),
                    st.used_placeholder,
                )
            };
            // The same marker Save Native writes, for the plain-export path --
            // a schedule whose masts were fabricated by the angle-table
            // reconstruction must say so in the file itself, not only in the app
            // that wrote it.
            if used_placeholder {
                indicatrix_formats::asc::mark_reconstructed(
                    &mut schedule,
                    "angle-table reconstruction, no attached .asc",
                );
            }
            // `to_asc_schedule_with` succeeding only means every tier
            // solved SOME mast, never that those masts actually close a real solid
            // -- see `decide_write_status`'s own doc comment. This path exports
            // plain `.asc` text only (`schedule`, already built above), never a
            // native sidecar.
            resolve_solved_then(&ui, Arc::new(design), move |ui, design, solved_result| {
                super::stall_guard::stall_guard("export_asc_status_resolved", || {
                    match decide_write_status(
                        &design,
                        solved_result.as_deref().map_err(String::as_str),
                    ) {
                        StatusDecision::Fine => finish_export_asc(ui, &schedule, default_file_name),
                        StatusDecision::NeedsConfirm(message) => {
                            let heading_message = format!(
                                "{message}\n\nExport anyway? The written file will note this in \
                                 its own header."
                            );
                            ask_write_confirm(
                                ui,
                                "This design is not a closed solid",
                                heading_message,
                                "Export Anyway",
                                Some(confirm_keys::NOT_CLOSED_SOLID),
                                move |ui| {
                                    let mut schedule = schedule;
                                    if let Some(header) =
                                        degenerate_marker_header(&schedule.headers, &message)
                                    {
                                        schedule.headers.insert(0, header);
                                    }
                                    finish_export_asc(ui, &schedule, default_file_name);
                                },
                            );
                        }
                    }
                });
            });
        });
    });
}

/// [`setup_export_asc_callback`]'s tail once the status question is settled
/// (clean, or confirmed with `schedule.headers` already stamped): picks a
/// destination ([`pick_file`], background thread) and writes it (background
/// thread).
fn finish_export_asc(
    ui: &MainWindow,
    schedule: &indicatrix_formats::asc::AscSchedule,
    default_file_name: String,
) {
    let text = indicatrix_formats::asc::to_asc_string(schedule);
    pick_file(
        ui,
        PickKind::SaveAsc {
            default_name: default_file_name,
        },
        move |ui, dest_path| {
            // A dismissed file dialog is the cutter's own deliberate cancel -- no
            // toast needed to confirm an action they just performed, and showing
            // one here could silently replace an error toast still waiting to be
            // read (see `gui::mod`'s own auto-dismiss scheduling).
            let Some(dest_path) = dest_path else {
                return;
            };
            let ui_weak = ui.as_weak();
            std::thread::spawn(move || {
                if let Some(parent) = dest_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let result = std::fs::write(&dest_path, &text)
                    .map_err(|e| format!("Failed to write {}: {e}", dest_path.display()));
                let _ = ui_weak.upgrade_in_event_loop(move |ui| match result {
                    Ok(()) => {
                        record_last_saved_path(&ui, &dest_path);
                        show_toast(
                            &ui,
                            &format!("Exported edited schedule to {}", dest_path.display()),
                            "success",
                        );
                    }
                    Err(message) => show_toast(&ui, &message, "error"),
                });
            });
        },
    );
}

/// "Save Native": writes the design's `.indicatrix.toml` sidecar ALONGSIDE a
/// real `.asc` export -- never instead of one. That pairing is the module's own hard
/// rule (see `indicatrix_cut_core::native`'s module doc comment): a design must never exist only
/// in a format its author can be locked out of, so this always writes both files,
/// never the native file alone.
///
/// `indicatrix_cut_core::save_paired` decides whether the `.asc` half can stay byte-identical to
/// `state.original_asc_text` (an untouched design, or one whose only edits were
/// `girdle_diameter_mm`/`material`/`preform` -- none of which round-trip into `.asc`
/// at all) or must be freshly regenerated -- see that function's own doc comment.
/// `state.asc_filename`/`original_asc_text` are `Some` only when this design's
/// schedule came from a real `.asc` on disk (a catalogue attachment via "Load
/// Selected", or a prior Save/Open Native -- see `EditorState::asc_filename`'s own
/// doc comment); `None` (a brand-new "New" design, or the angle-table placeholder
/// reconstruction) always regenerates, exactly like "Export .asc" already does.
///
/// Both fields are refreshed from what was ACTUALLY just written on success, so a
/// second Save right after the first preserves ITS OWN output rather than reopening
/// the preservation question against stale text.
///
/// Also wires up [`setup_dirty_tracking`] -- see that function's own doc comment for
/// why it is bundled in here rather than exported as its own `setup_*` entry point.
/// `native_path` is derived from `dest_path` (whatever `.asc` name the
/// cutter just picked through the OS's own save dialog, which already prompts for
/// THAT file on its own), never itself offered to the OS -- so a sidecar belonging to
/// a completely different design, sitting at this same derived path, would otherwise
/// be replaced with no prompt at all. Skipped (returns `true` with no dialog) when
/// `native_path` doesn't exist yet (nothing to overwrite) or already names this exact
/// state's own last save/open (re-saving your own file needs no confirmation --
/// `write_pair_atomically`'s own `.bak` step already covers that case). Split out of
/// [`setup_save_native_callback`] purely to keep that function under clippy's
/// line-count lint.
///
/// Uses [`ask_write_confirm`]'s in-window dialog instead of a blocking native
/// (`rfd` crate) message dialog. `on_confirmed` runs immediately (synchronously)
/// for the two cases that never needed asking at all (nothing to overwrite, or it
/// is this state's own file); otherwise it runs from the dialog's own accept
/// callback.
fn confirm_overwrite_unrelated_native_file_then(
    ui: &MainWindow,
    native_path: &Path,
    on_confirmed: impl FnOnce(&MainWindow) + 'static,
) {
    let is_own_file =
        CURRENT_NATIVE_PATH.with(|cell| cell.borrow().as_deref() == Some(native_path));
    if !native_path.is_file() || is_own_file {
        on_confirmed(ui);
        return;
    }
    ask_write_confirm(
        ui,
        "Overwrite existing file?",
        format!(
            "'{}' already exists and belongs to a different save (or a design this one \
             was never opened from). Overwriting it replaces that design's native \
             sidecar with this one's.",
            native_path.display()
        ),
        "Overwrite",
        Some(confirm_keys::OVERWRITE_UNRELATED_NATIVE),
        on_confirmed,
    );
}

/// This design's own already-known save location, when it was previously
/// saved to or opened from a real native pair. Without this, Ctrl+S/"Save Native"
/// would always reopen the Save-As dialog, even seconds after the cutter had just
/// chosen a location for it. `Some` only once BOTH halves of "we already know exactly where
/// this design lives" are true: [`CURRENT_NATIVE_PATH`] (the sidecar -- see its own
/// doc comment) and `asc_filename` (the paired `.asc`'s bare name, which
/// [`finish_save_native_success`] always records alongside it, in the very same
/// directory). `None` for a design that has never been saved/opened as a native pair
/// at all -- a brand-new "New" design, the angle-table placeholder reconstruction, or
/// a plain `.asc` opened with no sidecar ([`open_plain_asc`] clears
/// `CURRENT_NATIVE_PATH` precisely so this never fires for one) -- which still falls
/// through to [`save_native_via_dialog`]'s ordinary Save-As behaviour, unchanged.
fn known_save_target(st: &EditorState) -> Option<(PathBuf, PathBuf)> {
    let native_path = CURRENT_NATIVE_PATH.with(|cell| cell.borrow().clone())?;
    let asc_filename = st.asc_filename.as_deref()?;
    Some((native_path.with_file_name(asc_filename), native_path))
}

/// Quick save: writes straight to `dest_path`/`native_path` (both already
/// known -- see [`known_save_target`]) with no file dialog and no
/// [`confirm_overwrite_unrelated_native_file`] prompt (this IS this state's own file,
/// by construction of how the caller obtained these two paths). Otherwise identical
/// to [`save_native_via_dialog`]'s own tail: the same degenerate-status confirmation,
/// the same atomic pair write, the same success/failure reporting.
/// Every write/export/autosave path in this module that ends up calling
/// [`save_paired_reusing_solve`] needs this same bundle of `state` snapshot data
/// plus the destination paths and shared handles -- grouped into one `Clone`
/// struct (rather than nine-plus parameters threaded through several async
/// continuations) so [`finish_native_save`]/[`write_native_save`] can move a single
/// owned copy into whichever branch (confirmed or not) ends up running, and again
/// into the background thread that does the actual write.
#[derive(Clone)]
struct NativeSaveContext {
    state: Rc<RefCell<EditorState>>,
    dest_path: PathBuf,
    native_path: PathBuf,
    db: Arc<Mutex<Database>>,
    source: Arc<Mutex<LibrarySource>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    asc_filename: String,
    original_asc_text: Option<String>,
    printed_proportions: Option<ExternalProportions>,
    used_placeholder: bool,
    history_entries: Vec<String>,
}

thread_local! {
    /// [`write_native_save`]'s own UI-thread-only rendezvous for the
    /// `Rc<RefCell<EditorState>>` its background write thread must hand back to
    /// [`finish_save_native_success`] -- see [`pick_file`]'s own `PENDING_PICKS`
    /// doc comment for why a plain `Rc` can never itself cross into a spawned
    /// thread or an `upgrade_in_event_loop` closure.
    static PENDING_NATIVE_SAVE_STATE: RefCell<std::collections::HashMap<u64, Rc<RefCell<EditorState>>>> =
        RefCell::new(std::collections::HashMap::new());
    static NEXT_NATIVE_SAVE_KEY: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Group 1+2's shared tail for both [`quick_save_native`] and
/// [`save_native_via_dialog`] once `ctx.dest_path`/`ctx.native_path` are known and
/// any overwrite confirmation has already been granted: resolves `design`'s write
/// status off the UI thread ([`resolve_solved_then`]/[`decide_write_status`]), asks
/// [`ask_write_confirm`] only when it names a problem, then hands off to
/// [`write_native_save`].
fn finish_native_save(ui: &MainWindow, design: Design, ctx: NativeSaveContext) {
    resolve_solved_then(ui, Arc::new(design), move |ui, design, solved_result| {
        super::stall_guard::stall_guard("native_save_status_resolved", || {
            let solved = solved_result.as_ref().ok().cloned();
            match decide_write_status(&design, solved_result.as_deref().map_err(String::as_str)) {
                StatusDecision::Fine => {
                    write_native_save(ui, &design, solved.as_deref(), None, &ctx);
                }
                StatusDecision::NeedsConfirm(message) => {
                    let heading_message = format!(
                        "{message}\n\nSave anyway? The written file will note this in its own \
                         header."
                    );
                    ask_write_confirm(
                        ui,
                        "This design is not a closed solid",
                        heading_message,
                        "Save Anyway",
                        Some(confirm_keys::NOT_CLOSED_SOLID),
                        move |ui| {
                            write_native_save(ui, &design, solved.as_deref(), Some(&message), &ctx);
                        },
                    );
                }
            }
        });
    });
}

/// [`finish_native_save`]'s write half: builds the paired `.asc`/native TOML
/// (cheap -- no I/O, `save_paired_reusing_solve` never touches disk) on the UI
/// thread, then hands the actual disk write ([`write_pair_atomically`]) and
/// catalogue write-back ([`write_back_to_catalogue`]) to a background thread --
/// Group 4: "files first then catalogue, results back via the event loop."
/// `header_message` is `Some` only when the cutter just confirmed a "not a closed
/// solid" write; stamped into `design.meta.headers` before the sidecar's own
/// fingerprint is computed, so the fingerprint always describes the bytes actually
/// written, never mutated after the fact.
fn write_native_save(
    ui: &MainWindow,
    design: &Design,
    solved: Option<&[SolvedTier]>,
    header_message: Option<&str>,
    ctx: &NativeSaveContext,
) {
    let mut design = design.clone();
    if let Some(message) = header_message
        && let Some(header) = degenerate_marker_header(&design.meta.headers, message)
    {
        design.meta.headers.insert(0, header);
    }
    // A design reconstructed from a catalogue's bare angle table has every
    // mast fabricated as `0.0`. `save_paired` stamps
    // `indicatrix_formats::asc::mark_reconstructed` when told so, which is what stops
    // a file that looks like a real cut instruction from passing for one.
    let placeholder_note = ctx
        .used_placeholder
        .then_some("angle-table reconstruction, no attached .asc");
    // See `custom_material_snapshot_for_save`'s own doc comment.
    let custom_material = custom_material_snapshot_for_save(&design, &ctx.db);
    // Custom-catalogue-aware -- see
    // `snapshot_custom_materials`'s own doc comment.
    let custom_materials = snapshot_custom_materials(&ctx.render_ctx);
    let paired = match save_paired_reusing_solve(
        &design,
        solved,
        ctx.asc_filename.clone(),
        ctx.original_asc_text.as_deref(),
        placeholder_note,
        ctx.printed_proportions.as_ref(),
        &SaveExtras {
            custom_material: custom_material.as_ref(),
            history_entries: &ctx.history_entries,
            custom_catalogue: &custom_materials,
        },
    ) {
        Ok(paired) => paired,
        Err(e) => {
            show_toast(ui, &format!("Cannot save: {e}"), "error");
            return;
        }
    };

    let dest_path = ctx.dest_path.clone();
    let native_path = ctx.native_path.clone();
    let asc_filename = ctx.asc_filename.clone();
    let db = Arc::clone(&ctx.db);
    let db_for_finish = Arc::clone(&ctx.db);
    let source_for_finish = Arc::clone(&ctx.source);
    let source_entry_id = ctx.state.borrow().source_entry_id;
    // `Rc<RefCell<EditorState>>` is not `Send` -- it must never cross into the
    // spawned thread below or its `upgrade_in_event_loop` hop back. Stashed in a
    // UI-thread-only rendezvous instead, the same trick `pick_file`'s own
    // `PENDING_PICKS` uses (see that function's own doc comment) -- only the
    // plain `u64` key crosses the thread boundary.
    let state_key = NEXT_NATIVE_SAVE_KEY.with(|c| {
        let key = c.get();
        c.set(key + 1);
        key
    });
    PENDING_NATIVE_SAVE_STATE.with(|cell| {
        cell.borrow_mut().insert(state_key, Rc::clone(&ctx.state));
    });
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || {
        if let Some(parent) = dest_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let write_result = write_pair_atomically(
            &dest_path,
            &native_path,
            &paired.asc_text,
            &paired.native_toml,
        );
        let outcome = write_result.map(|()| {
            let native_filename = native_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            // Files-first-then-catalogue: a catalogue write-back failure below is
            // reported alongside the outcome, but never rolls back or re-reports
            // the file write that already succeeded -- see
            // `write_back_to_catalogue`'s own doc comment.
            let catalogue = write_back_to_catalogue(
                &db,
                source_entry_id,
                &asc_filename,
                &paired.asc_text,
                &native_filename,
                &paired.native_toml,
            );
            WriteNativeOutcome {
                dest_path: dest_path.clone(),
                native_path: native_path.clone(),
                asc_filename: asc_filename.clone(),
                paired,
                catalogue,
            }
        });
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            let Some(state) =
                PENDING_NATIVE_SAVE_STATE.with(|cell| cell.borrow_mut().remove(&state_key))
            else {
                return;
            };
            match outcome {
                Ok(outcome) => {
                    super::stall_guard::stall_guard("native_save_write_complete", || {
                        finish_save_native_success(
                            &ui,
                            &state,
                            outcome,
                            &db_for_finish,
                            &source_for_finish,
                        );
                    });
                }
                Err(message) => show_toast(&ui, &message, "error"),
            }
        });
    });
}

/// Quick save: writes straight to `ctx.dest_path`/`ctx.native_path`
/// (both already known -- see [`known_save_target`]) with no file dialog and no
/// [`confirm_overwrite_unrelated_native_file_then`] prompt (this IS this state's
/// own file, by construction of how the caller obtained these two paths).
/// Otherwise identical to [`save_native_via_dialog`]'s own tail: the same
/// degenerate-status confirmation, the same atomic pair write, the same
/// success/failure reporting -- both funnel through [`finish_native_save`].
fn quick_save_native(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    dest_path: &Path,
    native_path: &Path,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let (
        mut design,
        asc_filename,
        original_asc_text,
        printed_proportions,
        source_entry_id,
        used_placeholder,
        history_entries,
    ) = {
        let st = state.borrow();
        (
            st.design.clone(),
            // `known_save_target` already checked this is `Some`.
            st.asc_filename.clone().unwrap_or_default(),
            st.original_asc_text.clone(),
            st.printed_proportions,
            st.source_entry_id,
            st.used_placeholder,
            // Carried on every native save -- see `SaveExtras::history_entries`'s
            // own doc comment.
            st.history.description_log().to_vec(),
        )
    };
    // See `stamp_source_entry_footnote`'s own doc comment.
    stamp_source_entry_footnote(&mut design.meta.footnotes, source_entry_id);
    finish_native_save(
        ui,
        design,
        NativeSaveContext {
            state: Rc::clone(state),
            dest_path: dest_path.to_path_buf(),
            native_path: native_path.to_path_buf(),
            db: Arc::clone(db),
            source: Arc::clone(source),
            render_ctx: Arc::clone(render_ctx),
            asc_filename,
            original_asc_text,
            printed_proportions,
            used_placeholder,
            history_entries,
        },
    );
}

/// Save-As: always shows the native save dialog. Reached two ways: as
/// [`setup_save_native_callback`]'s own fallback, for a design
/// [`known_save_target`] has no answer for yet, and as `EditorModel.save_native_as`'s
/// entire body (a dedicated callback backed by its own
/// `ui/models/editor.slint`/`ui/app.slint` wiring) for a cutter who explicitly
/// wants to save the current design to a DIFFERENT file.
fn save_native_via_dialog(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let (
        mut design,
        default_asc_name,
        original_asc_text,
        printed_proportions,
        source_entry_id,
        used_placeholder,
        history_entries,
    ) = {
        let st = state.borrow();
        (
            st.design.clone(),
            suggested_file_name(&st),
            st.original_asc_text.clone(),
            st.printed_proportions,
            st.source_entry_id,
            st.used_placeholder,
            // Carried on every native save -- see `SaveExtras::history_entries`'s
            // own doc comment.
            st.history.description_log().to_vec(),
        )
    };
    // See `stamp_source_entry_footnote`'s own doc comment.
    stamp_source_entry_footnote(&mut design.meta.footnotes, source_entry_id);

    // Group 3: the save-as picker runs on a background thread -- see
    // `pick_file`'s own doc comment. `state` is not borrowed across this call.
    let state_for_pick = Rc::clone(state);
    let db_for_pick = Arc::clone(db);
    let source_for_pick = Arc::clone(source);
    let render_ctx_for_pick = Arc::clone(render_ctx);
    pick_file(
        ui,
        PickKind::SaveAsc {
            default_name: default_asc_name.clone(),
        },
        move |ui, dest_path| {
            // See the matching comment on `setup_export_asc_callback`'s own
            // save-picker cancel -- a dismissed dialog needs no toast.
            let Some(dest_path) = dest_path else {
                return;
            };
            let asc_filename = dest_path.file_name().map_or_else(
                || default_asc_name.clone(),
                |n| n.to_string_lossy().into_owned(),
            );
            let native_path = native_path_for_asc(&dest_path);

            // Group 2: the "overwrite an unrelated sidecar" confirmation is now the
            // shared in-window dialog too, never a blocking native one. Confirmed
            // against a clone: `native_path` itself is still borrowed for this very
            // call while `on_confirmed` is being constructed (it moves its own copy
            // in, for `NativeSaveContext`).
            let native_path_for_confirm = native_path.clone();
            confirm_overwrite_unrelated_native_file_then(ui, &native_path_for_confirm, move |ui| {
                finish_native_save(
                    ui,
                    design,
                    NativeSaveContext {
                        state: state_for_pick,
                        dest_path,
                        native_path,
                        db: db_for_pick,
                        source: source_for_pick,
                        render_ctx: render_ctx_for_pick,
                        asc_filename,
                        original_asc_text,
                        printed_proportions,
                        used_placeholder,
                        history_entries,
                    },
                );
            });
        },
    );
}

/// `db`/`source`: Save Native's own catalogue write-back needs them (see
/// [`write_back_to_catalogue`]), threaded in from
/// `gui::editor::setup_editor_callbacks`'s call site.
pub(super) fn setup_save_native_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    setup_dirty_tracking(ui, state);
    setup_autosave_timer(ui, state, db, render_ctx);
    // Group 2: the write-confirm dialog's own two callbacks -- bundled in here for
    // the same reason `setup_dirty_tracking`/`setup_autosave_timer` are.
    setup_write_confirm_dialog_callbacks(ui);
    let state_save = Rc::clone(state);
    let db_save = Arc::clone(db);
    let source_save = Arc::clone(source);
    let render_ctx_save = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_save_native(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        super::stall_guard::stall_guard("save_native", || {
            // Quick-save straight to the design's own known location when
            // one exists, so Ctrl+S/"Save Native" stop being a Save-As round trip
            // on every press -- only a design with no known location yet (see
            // `known_save_target`'s own doc comment) still shows the dialog.
            let known = {
                let st = state_save.borrow();
                known_save_target(&st)
            };
            match known {
                Some((dest_path, native_path)) => {
                    quick_save_native(
                        &ui,
                        &state_save,
                        &dest_path,
                        &native_path,
                        &db_save,
                        &source_save,
                        &render_ctx_save,
                    );
                }
                None => {
                    save_native_via_dialog(
                        &ui,
                        &state_save,
                        &db_save,
                        &source_save,
                        &render_ctx_save,
                    );
                }
            }
        });
    });

    // "Save Native As...": always shows the dialog, even when a known location
    // exists, for a cutter who explicitly wants this design written to a
    // DIFFERENT file. The plain `save_native` above deliberately does not offer
    // that choice, which is exactly why this exists.
    let state_save_as = Rc::clone(state);
    let db_save_as = Arc::clone(db);
    let source_save_as = Arc::clone(source);
    let render_ctx_save_as = Arc::clone(render_ctx);
    let ui_weak_as = ui.as_weak();
    ui.global::<EditorModel>().on_save_native_as(move || {
        let Some(ui) = ui_weak_as.upgrade() else {
            return;
        };
        super::stall_guard::stall_guard("save_native_as", || {
            save_native_via_dialog(
                &ui,
                &state_save_as,
                &db_save_as,
                &source_save_as,
                &render_ctx_save_as,
            );
        });
    });
}

/// Records where a Save/Export just wrote, for the status strip's own persistent
/// "Saved: ..." segment and its click-to-reveal. The strip
/// gets the bare file name (all a 26px strip has room for) and the full path
/// separately, for the Details popup and for the reveal itself -- see
/// `EditorModel.last_saved_path`'s own doc comment. A path with no file-name
/// component cannot come out of a save dialog, but is handled anyway by falling
/// back to the full path rather than clearing the label.
fn record_last_saved_path(ui: &MainWindow, path: &Path) {
    let full = path.display().to_string();
    let label = path
        .file_name()
        .map_or_else(|| full.clone(), |name| name.to_string_lossy().into_owned());
    let model = ui.global::<EditorModel>();
    model.set_last_saved_path(full.into());
    model.set_last_saved_label(label.into());
}

/// [`write_native_save`]'s background-thread result, once both the file write and
/// the catalogue write-back (Group 4: "files first then catalogue") have already
/// run -- everything [`finish_save_native_success`] needs to finish on the UI
/// thread. `catalogue` is its own `Result`, independent of the outer one this is
/// wrapped in by [`write_native_save`]: a catalogue failure never means the save
/// itself failed (see [`write_back_to_catalogue`]'s own doc comment), so it is
/// reported on its own rather than folded into a single combined error.
struct WriteNativeOutcome {
    dest_path: PathBuf,
    native_path: PathBuf,
    asc_filename: String,
    paired: indicatrix_cut_core::native::PairedSave,
    catalogue: Result<(i64, CatalogueWriteBack), String>,
}

/// [`write_native_save`]'s success tail, run back on the UI thread once its
/// background thread reports in -- split out purely to keep that function under
/// clippy's line-count lint. Drops any in-progress autosave (now strictly older
/// than what's actually on disk), records `native_path` as the most recent Open
/// Recent entry and this state's own file (see [`CURRENT_NATIVE_PATH`]'s doc
/// comment), updates `EditorState`'s own saved-generation bookkeeping, and toasts
/// the outcome -- a draft note when
/// [`indicatrix_cut_core::native::PairedSave::draft_reason`] is `Some`, an ordinary
/// success note otherwise -- then reports `outcome.catalogue`'s own result.
fn finish_save_native_success(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    outcome: WriteNativeOutcome,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let WriteNativeOutcome {
        dest_path,
        native_path,
        asc_filename,
        paired,
        catalogue,
    } = outcome;
    // The real save just landed -- any in-progress autosave is now strictly
    // older than what's on disk, so drop it rather than leaving a stale
    // recovery file behind (it never outlives the real save it stood in for).
    let _ = std::fs::remove_file(autosave_path(Some(asc_filename.as_str())));
    record_recent_native_file(ui, &native_path.display().to_string());
    CURRENT_NATIVE_PATH.with(|cell| *cell.borrow_mut() = Some(native_path.clone()));
    // Names the window title (`MainWindow.loaded_design_name`) after the file
    // that was just saved.
    ui.set_loaded_design_name(asc_filename.clone().into());
    // The `.asc` half, not the `.indicatrix.toml`: both land in the same folder
    // (so the reveal is identical either way) and the `.asc` is the one a cutter
    // hands to a machine or another program.
    record_last_saved_path(ui, &dest_path);

    {
        let mut st = state.borrow_mut();
        st.asc_filename = Some(asc_filename);
        st.original_asc_text = Some(paired.asc_text);
        // A draft save is still a real, successful save -- `design` not
        // currently solving does not fail `save_paired` at all (see
        // `PairedSave::draft_reason`'s own doc comment) -- so this is unconditional,
        // not only on the ordinary branch below.
        st.saved_generation = st.generation.load(Ordering::Relaxed);
    }
    ui.global::<EditorModel>()
        .set_is_dirty(state.borrow().is_dirty());

    // `draft_reason` is `Some` iff `design` did not currently solve, or had no
    // tiers at all, and this save fell back to a placeholder-mast `.asc` (see
    // `save_paired`'s own doc comment and `indicatrix_cut_core::native::DraftReason`)
    // -- worded so the cutter knows the file IS on disk but is not yet a real cut
    // instruction, rather than seeing a "Cannot save" error. `DraftReason`'s own
    // `Display` already names which of the two happened, so it is not repeated
    // here.
    if let Some(reason) = &paired.draft_reason {
        // This is a critical outcome -- the cutter's `.asc` is a placeholder, not
        // a real cut instruction -- so it gets the same persistent "warning" class
        // as the fingerprint-mismatch toast just below, not an "info" flash that
        // can vanish before it is read.
        show_toast(
            ui,
            &format!(
                "Saved '{}' as a draft ({reason}). Add a scale-reference tier to \
                 finish it.",
                dest_path.display()
            ),
            "warning",
        );
    } else {
        // The real condition (see
        // `crates/indicatrix-cut-core/src/native/save.rs`'s semantic-equality
        // gate) is that every meet note got resynthesized, which is what the
        // manual (docs/manual/11-saving-and-file-formats.md:57-61) documents --
        // the toast says so explicitly so it does not disagree with the manual on
        // how alarming this is.
        let asc_note = if paired.asc_preserved {
            "unchanged .asc preserved"
        } else {
            ".asc regenerated (meet notes rewritten)"
        };
        show_toast(
            ui,
            &format!(
                "Saved '{}' and '{}' ({asc_note}).",
                dest_path.display(),
                native_path.display()
            ),
            "success",
        );
    }

    // Registers (or updates) this design's own catalogue
    // row -- see `write_back_to_catalogue`'s own doc comment for exactly what gets
    // overwritten versus preserved. Already run, on a background thread, by
    // `write_native_save` (Group 4: "files first then catalogue, results back via
    // the event loop") -- this only reports `catalogue`'s own outcome. Never runs
    // after "Export .asc" (`setup_export_asc_callback`), which keeps its own
    // documented "file only, no database write of any kind" rule.
    match catalogue {
        Ok((id, catalogue_outcome)) => {
            // Every save after this one for a previously row-less (or
            // now-row-less, see `CatalogueWriteBack::SourceRowGoneNewRowCreated`)
            // design must update THIS row, never insert a second -- see
            // `EditorState::source_entry_id`'s own doc comment.
            state.borrow_mut().source_entry_id = Some(id);
            crate::gui::library::local::refresh_after_library_change(ui, db, source);
            match catalogue_outcome {
                CatalogueWriteBack::SourceRowGoneNewRowCreated => {
                    show_toast(
                        ui,
                        "This design's catalogue row no longer exists (it may have been \
                         deleted) -- saved as a new catalogue entry instead of updating it.",
                        "info",
                    );
                }
                // The design itself saved fine (this whole function only runs after
                // `write_pair_atomically` already succeeded) -- only the library's
                // cached previews/tilt curves for this row could not be cleared, so
                // the cutter needs to know not to trust them at a glance rather than
                // this failing silently into `tracing::warn!` alone.
                CatalogueWriteBack::UpdatedExisting {
                    stale_cache_invalidation_failed: true,
                } => {
                    show_toast(
                        ui,
                        "Saved, but this design's cached preview image and tilt curves in \
                         the library could not be cleared automatically -- they may still \
                         show the geometry from before this save. Re-export or recompute \
                         them if they look wrong.",
                        "warning",
                    );
                }
                CatalogueWriteBack::UpdatedExisting {
                    stale_cache_invalidation_failed: false,
                }
                | CatalogueWriteBack::NewRow => {}
            }
        }
        Err(message) => show_toast(ui, &format!("Catalogue not updated: {message}"), "error"),
    }
}

/// Writes `asc_text`/`native_toml` to `dest_path`/`native_path` as one
/// atomic-as-possible pair: a read-only folder, a full disk, or an
/// antivirus lock must never leave a plausible-looking `.asc` on disk with its
/// authored constraints nowhere to be found (this module's own stated rule, see
/// [`setup_save_native_callback`]'s doc comment).
///
/// Both files are staged under same-directory temp sibling names first ([`temp_sibling`]
/// -- always the same filesystem as the real target, so the rename that follows is a
/// cheap, effectively-atomic same-volume operation, never one that could silently
/// fall back to copy+delete across volumes). Before either temp file is renamed into
/// place, any file ALREADY at that destination is copied to a `.bak` sibling first
/// ([`backup_existing`]): a bad save (or this very save, if the design
/// regressed since the last one) must never overwrite the only copy of a design that
/// was already on disk. Only once both backups (when needed) and both writes have
/// already succeeded are the real renames attempted. The one residual failure window
/// -- the second rename failing after the first already landed -- is reported as
/// exactly that (`.asc` on disk, sidecar not yet written) rather than silently
/// claimed as a full success.
///
/// # Errors
///
/// A ready-to-toast message naming exactly what's on disk afterward.
fn write_pair_atomically(
    dest_path: &Path,
    native_path: &Path,
    asc_text: &str,
    native_toml: &str,
) -> Result<(), String> {
    let tmp_asc = temp_sibling(dest_path);
    let tmp_native = temp_sibling(native_path);

    if let Err(e) = std::fs::write(&tmp_asc, asc_text) {
        let _ = std::fs::remove_file(&tmp_asc);
        return Err(format!(
            "Failed to write {}: {e}. Nothing was saved.",
            dest_path.display()
        ));
    }
    if let Err(e) = std::fs::write(&tmp_native, native_toml) {
        let _ = std::fs::remove_file(&tmp_asc);
        let _ = std::fs::remove_file(&tmp_native);
        return Err(format!(
            "Failed to write {}: {e}. Nothing was saved.",
            native_path.display()
        ));
    }
    if let Err(message) = backup_existing(dest_path) {
        let _ = std::fs::remove_file(&tmp_asc);
        let _ = std::fs::remove_file(&tmp_native);
        return Err(message);
    }
    if let Err(message) = backup_existing(native_path) {
        let _ = std::fs::remove_file(&tmp_asc);
        let _ = std::fs::remove_file(&tmp_native);
        return Err(message);
    }
    if let Err(e) = std::fs::rename(&tmp_asc, dest_path) {
        let _ = std::fs::remove_file(&tmp_asc);
        let _ = std::fs::remove_file(&tmp_native);
        return Err(format!(
            "Failed to finalize {}: {e}. Nothing was saved.",
            dest_path.display()
        ));
    }
    if let Err(e) = std::fs::rename(&tmp_native, native_path) {
        return Err(format!(
            "Saved '{}' but failed to finalize its native sidecar {}: {e}. The .asc is on \
             disk without its native sidecar -- re-save once the problem is fixed.",
            dest_path.display(),
            native_path.display()
        ));
    }
    Ok(())
}

/// A same-directory temp sibling of `path`, used to stage a write before the final
/// atomic-as-possible rename -- see [`write_pair_atomically`]. Always a sibling
/// (never `std::env::temp_dir()`), so the rename that follows never crosses
/// filesystems.
fn temp_sibling(path: &Path) -> PathBuf {
    let file_name = path.file_name().map_or_else(
        || std::ffi::OsString::from("save.tmp"),
        |n| {
            let mut s = n.to_os_string();
            s.push(".tmp");
            s
        },
    );
    path.with_file_name(file_name)
}

/// `path`'s `.bak` sibling -- e.g. `design.asc` -> `design.asc.bak`, `design.
/// indicatrix.toml` -> `design.indicatrix.toml.bak`. One generation of backup only:
/// a second save in a row overwrites the `.bak` from the first, matching "keep the
/// PREVIOUS version" rather than an ever-growing history.
fn backup_sibling(path: &Path) -> PathBuf {
    let file_name = path.file_name().map_or_else(
        || std::ffi::OsString::from("save.bak"),
        |n| {
            let mut s = n.to_os_string();
            s.push(".bak");
            s
        },
    );
    path.with_file_name(file_name)
}

/// Never overwrites the only copy: copies `path` to its [`backup_sibling`]
/// before [`write_pair_atomically`] renames a freshly staged temp file over it. A
/// no-op (`Ok(())`) when `path` doesn't exist yet -- a design's first save has
/// nothing to back up. Copies rather than renames `path` itself: `path` is left
/// completely untouched by this step either way, so a failed backup aborts the whole
/// save (see [`write_pair_atomically`]) without having disturbed the file that was
/// already there.
///
/// # Errors
///
/// A ready-to-toast message naming the file that could not be backed up.
fn backup_existing(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    let backup = backup_sibling(path);
    std::fs::copy(path, &backup).map_err(|e| {
        format!(
            "Failed to back up {} to {} before overwriting it: {e}. Nothing was saved.",
            path.display(),
            backup.display()
        )
    })?;
    Ok(())
}

/// Wires `EditorModel.recompute_dirty` -- called by `changed tiers` in
/// `ui/models/editor.slint` every time ANY edit path (this app's own tier-editing
/// callbacks, but also Deep Solve/Optimize Apply, Adopt, and Retarget Apply, none of
/// which this module owns) rebuilds the tier list -- so [`EditorState::is_dirty`]
/// stays live without a `set_is_dirty` call at every one of those sites. Bundled into
/// [`setup_save_native_callback`] (called exactly once, like every other `setup_*`
/// entry point here) rather than given its own -- `callbacks::mod` only re-exports
/// entry points by name, and this one has no Slint button of its own to answer to.
fn setup_dirty_tracking(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_recompute_dirty(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        // `try_borrow`, never `borrow`: Slint runs `changed tiers` synchronously
        // from inside `set_tiers`, and the paths that push the tier list are
        // normally holding `state.borrow_mut()` while they do it (`do_new_design_
        // create`, `apply_loaded_design`, and every ordinary edit callback). A plain
        // `borrow()` panicked there with "already mutably borrowed".
        //
        // Skipping is safe rather than merely non-fatal: `view::
        // push_tier_list_and_undo_redo` -- the only thing that calls `set_tiers` --
        // pushes `is_dirty` itself from the `&EditorState` it already holds, on
        // every one of those paths. This handler exists for the pushes that do NOT
        // come through there, where nothing is borrowed and the read succeeds.
        if let Ok(st) = state.try_borrow() {
            ui.global::<EditorModel>().set_is_dirty(st.is_dirty());
        }
    });
}

/// How often the autosave timer checks [`EditorState::is_dirty`] and, if `true`,
/// writes a recovery snapshot. Two minutes:
/// frequent enough that a crash loses at most a couple of minutes of edits,
/// infrequent enough that it never shows up as a hitch even on a slow disk or a
/// large design. Gated on `is_dirty` (never writes while nothing has changed) and
/// never touches the cutter's own save path -- see [`autosave_path`]/
/// [`run_autosave_tick`].
const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(120);

thread_local! {
    /// The native `.indicatrix.toml` path this `EditorState` was last loaded from or
    /// saved to, if any -- `native_path` in [`setup_save_native_callback`]
    /// is DERIVED from whatever `.asc` name the cutter just picked
    /// ([`indicatrix_cut_core::native_path_for_asc`]), never itself chosen through the
    /// native save dialog, so the OS's own "this file already exists" prompt (which
    /// covers only the `.asc` half) never sees it. Comparing against this lets a Save
    /// tell "the same design's own sidecar, safe to overwrite" (already covered by
    /// [`write_pair_atomically`]'s own `.bak` backup) apart from "some unrelated
    /// design's sidecar that happens to share this `.asc` name," which gets an
    /// explicit native OS confirm instead. A plain `RefCell`, not part of
    /// `EditorState` itself: it names a location on disk, not design data, so it must
    /// not be reset by [`EditorState::replace_wholesale`] the way every other field on
    /// that struct is -- see that method's own doc comment.
    static CURRENT_NATIVE_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

thread_local! {
    /// Keeps the autosave `slint::Timer` alive for the life of the window -- a
    /// `Timer` stops as soon as its value is dropped, and this module owns no
    /// longer-lived struct to park the handle in (unlike `gui::mod`'s own
    /// `remote_rendering_timer` field). Mirrors `retarget_actions::RETARGET_ASYNC`'s
    /// own reasoning for using a `thread_local!` here: Slint's event loop is
    /// single-threaded, so this is sound without any real synchronization.
    static AUTOSAVE_TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

/// Starts the autosave timer -- bundled into [`setup_save_native_callback`] (called
/// exactly once, like every other `setup_*` entry point here) rather than given its
/// own, the same reasoning [`setup_dirty_tracking`] documents on itself.
///
/// # Recovery
///
/// The autosave lands at [`autosave_path`]: `<name>.indicatrix.autosave.toml` inside
/// this app's OWN settings directory (`settings::store::default_settings_path`'s
/// parent directory), never beside the cutter's real save location and never the
/// cutter's real `.asc`/native pair (this design's own path isn't tracked here).
/// After a crash, a cutter recovers by choosing File > Open Native and browsing to
/// that file directly; it opens exactly like any other native design file, picked
/// directly (not through [`read_picked_asc`]'s own bare-`.asc` path).
///
/// The autosave file is written by
/// [`indicatrix_cut_core::native::save_native_only`] (via [`run_autosave_tick`]),
/// which carries `design` in FULL -- every tier's real `angle_deg`/`indices` and
/// the `.asc`-only `meta` fields a native file otherwise never records at all (see
/// that function's own doc comment) -- so [`read_native_pair`]'s restore path can
/// call [`indicatrix_cut_core::native::load_native_only`] and rebuild the design
/// with NO paired `.asc` text needed, or even present on disk. A cutter recovers by
/// choosing File > Open Native and browsing to the autosave file directly; whether
/// `asc_filename`'s own file exists anywhere does not matter.
fn setup_autosave_timer(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let db = Arc::clone(db);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, AUTOSAVE_INTERVAL, move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        super::stall_guard::stall_guard("autosave_tick", || {
            run_autosave_tick(&ui, &state, &db, &render_ctx);
        });
    });
    AUTOSAVE_TIMER.with(|cell| *cell.borrow_mut() = Some(timer));
}

/// One autosave timer tick: writes a recovery snapshot iff [`EditorState::is_dirty`]
/// -- see [`setup_autosave_timer`]'s own doc comment.
///
/// Writes via [`indicatrix_cut_core::native::save_native_only_toml`] rather than
/// [`save_paired_reusing_solve`] -- a self-contained snapshot that carries every
/// tier's real `angle_deg`/`indices` directly, regardless of whether `design`
/// currently solves, so [`read_native_pair`]'s restore side never needs the
/// cutter's real `.asc` file (which may never have existed, or been moved/deleted)
/// to reopen it. Autosave needs no matching cached solve at all -- there is no
/// solve step to skip -- so every dirty tick produces a recoverable snapshot, not
/// just the ones lucky enough to land after a background solve completed. Carries
/// the same custom-material snapshot/history trail as a real Save Native so a
/// crash recovery does not itself reintroduce the "reopens as Diamond" bug.
///
/// Still skips the tick entirely while a background solve
/// (`EditorModel.solve_running`) is in flight, so this and that solve never stack
/// against the same `RenderContext`/database locks.
///
/// Group 4: the actual write ([`write_autosave`]) runs on a background thread,
/// never the UI thread; building the TOML string itself
/// (`save_native_only_toml`) is a plain, cheap in-memory serialization -- no
/// solving, no I/O -- so it stays on the UI thread like every other snapshot-then-
/// hand-to-a-worker call in this module.
fn run_autosave_tick(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    if ui.global::<EditorModel>().get_solve_running() {
        return;
    }
    let (design, asc_filename, printed_proportions, history_entries) = {
        let st = state.borrow();
        if !st.is_dirty() {
            return;
        }
        (
            st.design.clone(),
            st.asc_filename.clone(),
            st.printed_proportions,
            st.history.description_log().to_vec(),
        )
    };
    let custom_material = custom_material_snapshot_for_save(&design, db);
    // Custom-catalogue-aware -- see
    // `snapshot_custom_materials`'s own doc comment.
    let custom_materials = snapshot_custom_materials(render_ctx);
    let Ok(native_toml) = save_native_only_toml(
        &design,
        format!("{}.asc", autosave_base_name(asc_filename.as_deref())),
        printed_proportions.as_ref(),
        &SaveExtras {
            custom_material: custom_material.as_ref(),
            history_entries: &history_entries,
            custom_catalogue: &custom_materials,
        },
    ) else {
        // `SaveError::Toml` is unreachable in practice (see its own doc comment) and
        // there is nothing actionable to toast for a silent background autosave.
        return;
    };
    let path = autosave_path(asc_filename.as_deref());
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || {
        let result = write_autosave(&path, &native_toml);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            if let Err(e) = result {
                show_toast(
                    &ui,
                    &format!("Autosave to {} failed: {e}", path.display()),
                    "error",
                );
            }
        });
    });
}

/// The base name an autosave is filed under: the design's own `asc_filename` (its
/// bare `.asc` name) with the extension stripped, or `"untitled"` for a design never
/// yet paired with one (a brand-new design, or the angle-table placeholder
/// reconstruction).
fn autosave_base_name(asc_filename: Option<&str>) -> String {
    let name = asc_filename.unwrap_or("untitled");
    name.strip_suffix(".asc").unwrap_or(name).to_string()
}

/// Where [`run_autosave_tick`] writes: `<name>.indicatrix.autosave.toml` inside this
/// app's own settings directory, falling back to the OS temp directory on the rare
/// platform where even that can't be resolved -- either way, never a location the
/// cutter chose or the design's own paired files live at.
fn autosave_path(asc_filename: Option<&str>) -> PathBuf {
    let dir = crate::settings::store::default_settings_path()
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf);
    dir.join(format!(
        "{}.indicatrix.autosave.toml",
        autosave_base_name(asc_filename)
    ))
}

/// This is the startup check for a leftover autosave file, exposed for
/// `gui::editor::mod`'s startup sequence to call before [`EditorState::fresh`]
/// replaces whatever the previous session had. Without it, a crash still loses the
/// session in practice even though the autosave file itself is written, since
/// nothing checks for one or prompts to restore it on launch.
///
/// Scans [`autosave_path`]'s own directory (never recurses -- there is nothing to
/// recurse into, every autosave lands flat in this one app-settings directory) for
/// any `*.indicatrix.autosave.toml` file and returns the most recently modified one,
/// if any. Does not open or delete anything itself: a leftover file only means SOME
/// past session was dirty when it stopped ticking (a real crash, or simply the
/// window closing between one 120-second tick and the next), never that the design
/// in it is still wanted -- the caller decides whether to offer it and, either way,
/// to remove it afterward (matching [`setup_save_native_callback`]'s own
/// successful-save cleanup, just above).
#[must_use]
pub(super) fn find_leftover_autosave() -> Option<PathBuf> {
    let dir = crate::settings::store::default_settings_path()
        .parent()
        .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf);
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".indicatrix.autosave.toml"))
        })
        .max_by_key(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok())
}

/// Writes `native_toml` to `path` via the same stage-then-rename discipline
/// [`write_pair_atomically`] uses, so a crash mid-autosave-write can never leave a
/// half-written, corrupt recovery file behind either.
///
/// # Errors
///
/// A ready-to-toast message.
fn write_autosave(path: &Path, native_toml: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = temp_sibling(path);
    std::fs::write(&tmp, native_toml).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

/// "Open Recent" write side: records `native_path_display` as the
/// most-recently-used native file in the settings store
/// ([`crate::settings::model::AppSettings::record_recent_native_file`]) and
/// refreshes `MainWindow.recent_native_files` so File > Open Recent reflects it
/// immediately, with no restart needed. Called after every successful Save Native
/// and Open Native (both the picker and File > Open Recent itself, since both funnel
/// through [`commit_loaded_native`]) -- never for [`open_plain_asc`]'s bare-`.asc`
/// path, which has no native file to name.
///
/// Reads/writes the settings file directly via [`crate::settings::store`] rather
/// than through the debounced [`crate::settings::SettingsPersister`]: this module has
/// no handle to it (`gui::editor::setup_editor_callbacks` doesn't thread one in, and
/// adding one would be a `gui::editor::mod`/`gui::mod` change). This is a real, if
/// narrow, race as a result: a settings change still
/// inside the persister's ~600ms debounce window when this runs writes its own
/// (older, recent-files-less) in-memory snapshot over this one the next time it
/// flushes, silently dropping the just-recorded entry. Accepted rather than leaving
/// the whole feature unwired -- the consequence is losing one recent-files entry
/// occasionally, never any design data, and the entry reappears next save/open
/// anyway.
fn record_recent_native_file(ui: &MainWindow, native_path_display: &str) {
    let settings_path = crate::settings::store::default_settings_path();
    let mut file = crate::settings::store::load_or_default(&settings_path);
    file.settings
        .record_recent_native_file(native_path_display.to_string());
    let _ = crate::settings::store::save(&settings_path, &file);
    ui.set_recent_native_files(ModelRc::new(VecModel::from(
        file.settings
            .recent_native_files
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
}

/// "Open Native": loads a `.indicatrix.toml` (or legacy `.gemcut.toml`) sidecar
/// together with its paired `.asc` via `indicatrix_cut_core::load_paired`. Replaces the whole editor state the same way
/// "New"/"Load Selected" do (fresh `History`, no printed proportions -- a locally
/// opened native file has no catalogue row to verify Deep Solve against, exactly
/// like a brand-new design).
///
/// Checks [`EditorState::is_dirty`] BEFORE doing anything else -- including before
/// showing the native-file picker -- exactly like `setup_new_design_create_callback`/
/// `setup_load_selected_callback` do for New/Load Selected: asking "keep unsaved
/// changes?" only after making the user pick a file would be backwards. A dirty
/// design stashes [`PendingUnsavedAction::OpenNative`] and opens the guard dialog
/// instead of proceeding; [`setup_unsaved_guard_dispatch`]
/// (`gui::editor::callbacks::tier_actions`) resumes by calling [`do_open_native`]
/// again once Save/Discard is chosen, which re-shows the picker from scratch.
///
/// Also registers the fingerprint-mismatch dialog's three callbacks
/// ([`PENDING_MISMATCH`]) -- bundled in here rather than a separate `setup_*` for the
/// same reason [`setup_dirty_tracking`] is bundled into [`setup_save_native_callback`]:
/// this is the one `setup_*` entry point Open Native's own wiring has to answer to.
pub(super) fn setup_open_native_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
) {
    setup_mismatch_dialog_callbacks(ui, state, render_ctx, preview_state, solid_last_solved);

    let state_open = Rc::clone(state);
    let render_ctx_open = Arc::clone(render_ctx);
    let preview_state_open = Arc::clone(preview_state);
    let solid_last_solved_open = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_open_native(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        super::stall_guard::stall_guard("open_native", || {
            if state_open.borrow().is_dirty() {
                state_open.borrow_mut().pending_unsaved_action =
                    Some(PendingUnsavedAction::OpenNative);
                ui.global::<EditorModel>().set_unsaved_dialog_message(
                    "Opening a native file will discard the current design's unsaved changes."
                        .into(),
                );
                ui.global::<EditorModel>().set_unsaved_dialog_open(true);
                return;
            }
            do_open_native(
                &ui,
                &state_open,
                &render_ctx_open,
                &preview_state_open,
                &solid_last_solved_open,
            );
        });
    });

    // File > Open Recent (`ui/app.slint`'s `MainWindow.open_recent_native_file`) --
    // a root-component callback rather than an `EditorModel` one, since
    // `recent_native_files`/`open_recent_native_file` are declared directly on
    // `MainWindow` (`ui/app.slint`) rather than on
    // `EditorModel` (`ui/models/editor.slint`, owned elsewhere). Opening an entry
    // re-records it via [`record_recent_native_file`] (inside
    // [`commit_loaded_native`], which this path shares with the ordinary picker),
    // so using a recent file also bumps it back to the front. Bundled into this
    // `setup_*` entry point for the same reason `setup_mismatch_dialog_callbacks`
    // is, immediately above.
    let state_recent = Rc::clone(state);
    let render_ctx_recent = Arc::clone(render_ctx);
    let preview_state_recent = Arc::clone(preview_state);
    let solid_last_solved_recent = Arc::clone(solid_last_solved);
    let ui_weak_recent = ui.as_weak();
    ui.on_open_recent_native_file(move |path| {
        let Some(ui) = ui_weak_recent.upgrade() else {
            return;
        };
        super::stall_guard::stall_guard("open_recent_native_file", || {
            open_recent_native_path(
                &ui,
                &state_recent,
                &render_ctx_recent,
                &preview_state_recent,
                &solid_last_solved_recent,
                PathBuf::from(path.as_str()),
            );
        });
    });
}

/// File > Open Recent's own click handler -- loads `native_path` directly (no file
/// picker) via the exact same [`read_native_pair`]/[`open_native_pair`] path
/// [`do_open_native`] uses for a picked `.toml`. Guards on
/// [`EditorState::is_dirty`] like every other destructive replace-the-design entry
/// point in this module, but -- unlike [`setup_open_native_callback`]'s own picker
/// path -- does not yet resume through the Save/Discard/Cancel dialog on a dirty
/// design: `PendingUnsavedAction` has no variant carrying a specific path to resume
/// at (only `OpenNative`, which re-shows the picker from scratch), and that enum is
/// owned by `state/mod.rs`. Refusing with a toast instead is safe (nothing is lost)
/// though less smooth; a future change could close this gap by adding a
/// `PendingUnsavedAction` variant that carries a specific path to resume at.
///
/// `pub(super)` (rather than private) so `gui::editor::mod`'s startup sequence can
/// reuse it for "reopen last design" -- `AppSettings::recent_native_files`
/// already carries the most-recently-used path first; this is the same load path
/// "File > Open Recent" itself uses to open it.
pub(super) fn open_recent_native_path(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
    native_path: PathBuf,
) {
    if state.borrow().is_dirty() {
        show_toast(
            ui,
            "Save or discard the current design's unsaved changes before opening a recent file.",
            "error",
        );
        return;
    }
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    // `native_path` itself is still borrowed for this very call while `on_done` is
    // being constructed below (it moves its own copy in).
    let native_path_for_read = native_path.clone();
    read_native_pair_then(ui, &native_path_for_read, move |ui, result| {
        match result {
            Some(NativePairOrSelfContained::Pair {
                parsed_native,
                asc_text,
                native_text,
            }) => open_native_pair(
                ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                PickedPair {
                    native_path,
                    parsed_native,
                    asc_text,
                    native_text,
                },
            ),
            // The same autosave-restore fallback `do_open_native` gets, reached
            // here too since a leftover autosave file is reopened through this
            // same function
            // (`gui::editor::mod`'s startup sequence calls this with
            // `find_leftover_autosave`'s own path).
            Some(NativePairOrSelfContained::SelfContained {
                loaded,
                asc_filename,
            }) => open_native_self_contained(
                ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                SelfContainedLoad {
                    native_path: &native_path,
                    loaded: *loaded,
                    asc_filename: &asc_filename,
                },
            ),
            None => {}
        }
    });
}

/// The native-load fingerprint-mismatch choice's stashed inputs -- everything
/// [`PENDING_MISMATCH`]'s three resolution callbacks need to either recommit the
/// already-loaded (overlay-skipped) design or re-run [`load_paired`] with
/// `apply_overlay_on_mismatch: true`. Deliberately just the two source texts plus the
/// display path/bare filename, not the whole [`indicatrix_cut_core::LoadPairedResult`]
/// or parsed [`indicatrix_cut_core::NativeDesignFile`] -- re-parsing both (cheap: this
/// only happens once, on the user's own explicit click) is simpler than keeping a
/// second, slightly-different-shaped snapshot of the same two files in sync.
struct PendingMismatch {
    native_path_display: String,
    asc_filename: String,
    asc_text: String,
    native_text: String,
}

thread_local! {
    /// Stashed by [`do_open_native`] the moment it sees
    /// [`indicatrix_cut_core::TierOverlay::SkippedFingerprintMismatch`], read back by
    /// whichever of [`setup_mismatch_dialog_callbacks`]'s three handlers the user
    /// picks. `None` whenever the mismatch dialog is closed (the common state) --
    /// mirrors `tier_actions::REMOTE_LOAD_TARGET`'s own `thread_local!` shape, though
    /// for a different reason: this is plain, non-`Send` local data with nothing
    /// forcing a `thread_local!` on its own, but it still needs to outlive the single
    /// `do_open_native` call that stashes it, across however long the user takes to
    /// click a button, which a local variable can't do.
    static PENDING_MISMATCH: RefCell<Option<PendingMismatch>> = const { RefCell::new(None) };
}

/// What the "Open Native" picker actually returned -- see [`setup_open_native_callback`]'s
/// own doc comment for why the same picker also accepts a bare `.asc` (a
/// cutter handed a plain `GemCAD` file by email should not have to import it into the
/// catalogue database first just to look at it).
enum PickedNative {
    /// A real native+`.asc` pair, ready for [`load_paired`]. Boxed: [`PickedPair`]
    /// carries a whole parsed [`NativeDesignFile`] plus both source texts, several
    /// times larger than [`Self::AscOnly`]'s bare path+text
    /// (`clippy::large_enum_variant`).
    Pair(Box<PickedPair>),
    /// A directly-picked bare `.asc` with no native sidecar found next to it --
    /// nothing [`load_paired`] has any use for (no per-tier overlay, no fingerprint,
    /// no draft flag); committed straight through
    /// `gui::editor::loading::design_from_asc_text` instead, in [`open_plain_asc`].
    AscOnly { asc_path: PathBuf, asc_text: String },
    /// A native file with NO paired `.asc` findable anywhere (recorded name,
    /// naming-guess, and -- unlike the other two variants -- no prompt for one
    /// either), opened via [`load_native_only`] instead: the autosave-restore case
    /// [`save_native_only`](indicatrix_cut_core::native::save_native_only)'s own
    /// doc comment describes. [`read_native_pair_then`] only ever produces this
    /// when the file actually parses as self-contained -- an ordinary native file
    /// with a genuinely missing `.asc` still falls through to
    /// [`resolve_paired_asc_text_then`]'s "Locate the paired .asc" prompt, since
    /// only a [`save_native_only`](indicatrix_cut_core::native::save_native_only)
    /// file carries enough to skip it. Boxed for the same
    /// `clippy::large_enum_variant` reason as [`Self::Pair`].
    SelfContained {
        native_path: PathBuf,
        loaded: Box<LoadNativeOnlyResult>,
        asc_filename: String,
    },
}

/// [`PickedNative::Pair`]'s payload -- bundled into its own struct (rather than four
/// more parameters on [`open_native_pair`]) purely to keep that function under
/// clippy's argument-count lint, the same reasoning [`LoadedNativeOutcome`] uses.
struct PickedPair {
    native_path: PathBuf,
    parsed_native: Box<NativeDesignFile>,
    asc_text: String,
    native_text: String,
}

/// [`read_native_pair_then`]'s outcome -- a real pair (the ordinary case), or a
/// self-contained native file with no paired `.asc` involved at all. Kept
/// distinct from [`PickedNative`] itself (rather than reusing it directly)
/// since neither of `read_native_pair_then`'s
/// two callers has a `native_path` to attach until this returns.
enum NativePairOrSelfContained {
    Pair {
        // Boxed: `clippy::large_enum_variant` against `SelfContained`'s own,
        // much smaller payload.
        parsed_native: Box<NativeDesignFile>,
        asc_text: String,
        native_text: String,
    },
    SelfContained {
        loaded: Box<LoadNativeOnlyResult>,
        asc_filename: String,
    },
}

/// Reads/parses a native file already known to live at `native_path`, plus its
/// recorded paired `.asc` -- split out of [`pick_native_or_asc_then`]/
/// [`read_picked_asc_then`] purely to keep both under clippy's line-count lint.
/// `on_done`'s `None` is any read/parse failure, each already toasted here before
/// calling it.
///
/// [`resolve_paired_asc_text_then`]'s own "Locate the paired .asc" recovery picker
/// runs off the UI thread, so this (and every caller up the chain) is
/// continuation-passing too. The `std::fs::read_to_string`/TOML-parse calls
/// themselves stay synchronous -- reading one small file is cheap enough not to
/// need the same treatment.
///
/// Before ever prompting for the paired `.asc`, tries [`load_native_only`] on
/// `native_text` iff neither the recorded nor the guessed `.asc` path exists on
/// disk -- see [`NativePairOrSelfContained::SelfContained`]'s own doc comment.
/// Almost every native file fails that check immediately ([`load_native_only`]
/// itself refuses anything that isn't a
/// [`save_native_only`](indicatrix_cut_core::native::save_native_only) file), so
/// an ordinary design with a genuinely missing `.asc` still reaches
/// [`resolve_paired_asc_text_then`]'s prompt.
fn read_native_pair_then(
    ui: &MainWindow,
    native_path: &Path,
    on_done: impl FnOnce(&MainWindow, Option<NativePairOrSelfContained>) + 'static,
) {
    let native_text = match std::fs::read_to_string(native_path) {
        Ok(text) => text,
        Err(e) => {
            show_toast(
                ui,
                &format!("Failed to read {}: {e}", native_path.display()),
                "error",
            );
            on_done(ui, None);
            return;
        }
    };
    // The native file's own recorded `asc_filename` (a bare file name -- see
    // `indicatrix_cut_core::NativeDesignFile::asc_filename`'s own doc comment) is read
    // AFTER parsing the chosen file, since that field is the authoritative pointer to
    // the real paired file once a native file has actually been parsed; this never
    // guesses at a sibling `.asc` name the way `indicatrix_cut_core::asc_path_for_native`
    // does for a picker's initial directory (there is no picker here to seed -- the
    // native file's own directory plus its own recorded name is exact).
    let parsed_native = match indicatrix_cut_core::native::parse_toml_string(&native_text) {
        Ok(n) => Box::new(n),
        Err(e) => {
            show_toast(
                ui,
                &format!(
                    "'{}' is not a valid native design file: {e}",
                    native_path.display()
                ),
                "error",
            );
            on_done(ui, None);
            return;
        }
    };
    let asc_path = native_path.with_file_name(&parsed_native.asc_filename);
    // Cloned so `resolve_paired_asc_text_then`'s borrow of it can end before this
    // function's own `move` closure below takes ownership of `parsed_native` (whose
    // own field it would otherwise still be borrowing).
    let asc_filename = parsed_native.asc_filename.clone();

    let asc_findable = asc_path.is_file()
        || indicatrix_cut_core::asc_path_for_native(native_path)
            .is_some_and(|guessed| guessed != asc_path && guessed.is_file());
    // Not self-contained (an ordinary native file whose `.asc` genuinely went
    // missing) falls through to the same prompt this always showed.
    if !asc_findable && let Ok(loaded) = load_native_only(&native_text) {
        on_done(
            ui,
            Some(NativePairOrSelfContained::SelfContained {
                loaded: Box::new(loaded),
                asc_filename,
            }),
        );
        return;
    }

    resolve_paired_asc_text_then(
        ui,
        native_path,
        &asc_path,
        &asc_filename,
        move |ui, asc_text| {
            on_done(
                ui,
                asc_text.map(|asc_text| NativePairOrSelfContained::Pair {
                    parsed_native,
                    asc_text,
                    native_text,
                }),
            );
        },
    );
}

/// Reads the paired `.asc`'s text, recovering from a moved/renamed file (common
/// after `GemCad`'s own Save As) instead of giving up outright. Tries, in
/// order: `recorded_asc_path` (the native file's own authoritative
/// `asc_filename`, exact when it still holds); [`indicatrix_cut_core::asc_path_for_native`]'s
/// naming guess (only meaningful when it names a DIFFERENT path -- most native files'
/// own recorded name already matches the guess, so this rarely fires on its own); and
/// finally an explicit "Locate the paired .asc" picker (Group 3: [`pick_file`], on a
/// background thread), filtered to `*.asc`, so a cutter who renamed or moved the file
/// can point at it directly rather than hand-editing the sidecar's TOML. `on_done`'s
/// `None` (already toasted) only once all three have failed or the picker was
/// cancelled.
fn resolve_paired_asc_text_then(
    ui: &MainWindow,
    native_path: &Path,
    recorded_asc_path: &Path,
    recorded_asc_filename: &str,
    on_done: impl FnOnce(&MainWindow, Option<String>) + 'static,
) {
    if let Ok(text) = std::fs::read_to_string(recorded_asc_path) {
        on_done(ui, Some(text));
        return;
    }
    if let Some(guessed) = indicatrix_cut_core::asc_path_for_native(native_path)
        && guessed != recorded_asc_path
        && let Ok(text) = std::fs::read_to_string(&guessed)
    {
        on_done(ui, Some(text));
        return;
    }
    show_toast(
        ui,
        &format!(
            "'{}' names a paired .asc file '{recorded_asc_filename}', but it could not be found \
             next to it. Locate it to continue.",
            native_path.display()
        ),
        "info",
    );
    pick_file(ui, PickKind::LocateAsc, move |ui, located| {
        let Some(located) = located else {
            // The cutter's own explicit cancel -- no toast.
            on_done(ui, None);
            return;
        };
        match std::fs::read_to_string(&located) {
            Ok(text) => on_done(ui, Some(text)),
            Err(e) => {
                show_toast(
                    ui,
                    &format!("Failed to read {}: {e}", located.display()),
                    "error",
                );
                on_done(ui, None);
            }
        }
    });
}

/// A directly-picked `.asc` file: checks for a sibling native sidecar first (via
/// `native_path_for_asc`), so a pair still opens as a full pair even when picked by
/// its `.asc` half -- preserving the authored meet constraints/detached facets a bare
/// `.asc` re-import would otherwise drop entirely. Falls back to
/// [`PickedNative::AscOnly`] only when no sidecar file exists at all; a sidecar that
/// exists but fails to read/parse is a real error (already toasted by
/// [`read_native_pair_then`]), not silently skipped.
/// Wraps [`read_native_pair_then`]'s outcome into a [`PickedNative`] for either
/// caller below -- both need this exact translation and differ only in where
/// `native_path` itself came from (a naming guess vs. the cutter's own picker
/// choice).
fn picked_native_from_pair_result(
    native_path: PathBuf,
    outcome: NativePairOrSelfContained,
) -> PickedNative {
    match outcome {
        NativePairOrSelfContained::Pair {
            parsed_native,
            asc_text,
            native_text,
        } => PickedNative::Pair(Box::new(PickedPair {
            native_path,
            parsed_native,
            asc_text,
            native_text,
        })),
        NativePairOrSelfContained::SelfContained {
            loaded,
            asc_filename,
        } => PickedNative::SelfContained {
            native_path,
            loaded,
            asc_filename,
        },
    }
}

fn read_picked_asc_then(
    ui: &MainWindow,
    asc_path: PathBuf,
    on_done: impl FnOnce(&MainWindow, Option<PickedNative>) + 'static,
) {
    let native_path = native_path_for_asc(&asc_path);
    if native_path.is_file() {
        // `native_path` itself is still borrowed for this very call while
        // `on_done` is being constructed below (it moves its own copy in).
        let native_path_for_read = native_path.clone();
        read_native_pair_then(ui, &native_path_for_read, move |ui, result| {
            on_done(
                ui,
                result.map(|outcome| picked_native_from_pair_result(native_path, outcome)),
            );
        });
        return;
    }

    let asc_text = match std::fs::read_to_string(&asc_path) {
        Ok(text) => text,
        Err(e) => {
            show_toast(
                ui,
                &format!("Failed to read {}: {e}", asc_path.display()),
                "error",
            );
            on_done(ui, None);
            return;
        }
    };
    on_done(ui, Some(PickedNative::AscOnly { asc_path, asc_text }));
}

/// Shows the "Open Native" picker (accepting `.toml` OR `.asc`) and reads
/// whichever one the cutter picked. `on_done`'s `None` is a cancellation or any
/// read/parse failure, each already toasted before calling it.
///
/// The picker itself ([`PickKind::OpenNativeOrAsc`]) runs on a background
/// thread via [`pick_file`] -- see that function's own `match` for the filter
/// shape it builds.
fn pick_native_or_asc_then(
    ui: &MainWindow,
    on_done: impl FnOnce(&MainWindow, Option<PickedNative>) + 'static,
) {
    pick_file(ui, PickKind::OpenNativeOrAsc, move |ui, picked_path| {
        let Some(picked_path) = picked_path else {
            // A dismissed file dialog is the cutter's own deliberate cancel --
            // no toast needed.
            on_done(ui, None);
            return;
        };
        if picked_path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("asc"))
        {
            read_picked_asc_then(ui, picked_path, on_done);
            return;
        }
        // `picked_path` itself is still borrowed for this very call while
        // `on_done` is being constructed below (it moves its own copy in).
        let picked_path_for_read = picked_path.clone();
        read_native_pair_then(ui, &picked_path_for_read, move |ui, result| {
            on_done(
                ui,
                result.map(|outcome| picked_native_from_pair_result(picked_path, outcome)),
            );
        });
    });
}

/// The actual "Open Native" work -- see [`setup_open_native_callback`]'s own doc
/// comment for why the unsaved-changes guard runs before this is ever called, not
/// inside it. Reads whichever file the cutter picked via [`pick_native_or_asc_then`],
/// then dispatches to [`open_native_pair`] (a real pair) or [`open_plain_asc`] (a
/// bare `.asc`, no sidecar).
pub(in crate::gui::editor) fn do_open_native(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    pick_native_or_asc_then(ui, move |ui, picked| match picked {
        Some(PickedNative::Pair(picked)) => {
            open_native_pair(
                ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                *picked,
            );
        }
        Some(PickedNative::AscOnly { asc_path, asc_text }) => open_plain_asc(
            ui,
            &state,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            &asc_path,
            &asc_text,
        ),
        Some(PickedNative::SelfContained {
            native_path,
            loaded,
            asc_filename,
        }) => open_native_self_contained(
            ui,
            &state,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            SelfContainedLoad {
                native_path: &native_path,
                loaded: *loaded,
                asc_filename: &asc_filename,
            },
        ),
        None => {}
    });
}

/// The real native+`.asc` pair path -- split out of [`do_open_native`] purely to keep
/// that function under clippy's line-count/argument-count lints. `false` passed to
/// [`load_paired`]: never silently keep a per-tier overlay whose fingerprint no
/// longer matches the `.asc` it would be applied against -- see [`TierOverlay`]'s own
/// doc comment. A `SkippedFingerprintMismatch` result pauses on the mismatch dialog
/// instead of committing it; every other outcome (including the defensive-only
/// `SkippedTierCountMismatch`, never expected in practice per its own doc comment)
/// commits immediately via [`commit_loaded_native`].
fn open_native_pair(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
    picked: PickedPair,
) {
    let PickedPair {
        native_path,
        parsed_native,
        asc_text,
        native_text,
    } = picked;
    match load_paired(&asc_text, &native_text, false) {
        Ok(loaded) => {
            if matches!(loaded.tier_overlay, TierOverlay::SkippedFingerprintMismatch) {
                // Only offered when the native file's own tier count still agrees
                // with the freshly imported `.asc` -- see
                // `EditorModel.mismatch_dialog_can_apply`'s own doc comment. Recomputed
                // here from `loaded.design.tiers` (the `.asc`-derived tier list, before
                // any overlay) rather than trusted from `loaded.tier_overlay` itself,
                // since `SkippedFingerprintMismatch` alone doesn't distinguish the two.
                let can_apply = parsed_native.tiers.len() == loaded.design.tiers.len();
                PENDING_MISMATCH.with(|cell| {
                    *cell.borrow_mut() = Some(PendingMismatch {
                        native_path_display: native_path.display().to_string(),
                        asc_filename: parsed_native.asc_filename,
                        asc_text,
                        native_text,
                    });
                });
                ui.global::<EditorModel>()
                    .set_mismatch_dialog_can_apply(can_apply);
                ui.global::<EditorModel>().set_mismatch_dialog_open(true);
                return;
            }
            let is_mismatch = !matches!(loaded.fingerprint, FingerprintCheck::Match);
            commit_loaded_native(
                ui,
                state,
                render_ctx,
                preview_state,
                solid_last_solved,
                LoadedNativeOutcome {
                    loaded,
                    native_path_display: native_path.display().to_string(),
                    asc_filename: parsed_native.asc_filename,
                    asc_text,
                    is_mismatch,
                },
            );
        }
        Err(e) => show_toast(ui, &format!("Cannot open: {e}"), "error"),
    }
}

/// [`open_native_self_contained`]'s own load result -- bundled (rather than
/// three more parameters) purely to keep that function under clippy's
/// argument-count lint, the same reasoning [`LoadedNativeOutcome`] uses.
struct SelfContainedLoad<'a> {
    native_path: &'a Path,
    loaded: LoadNativeOnlyResult,
    asc_filename: &'a str,
}

/// The self-contained-native-file
/// path -- [`read_native_pair_then`] only ever hands this a [`LoadNativeOnlyResult`]
/// once it has already confirmed no paired `.asc` was findable AND
/// [`load_native_only`] actually accepted the file, so there is no fingerprint/
/// tier-overlay question to ask here at all (unlike [`open_native_pair`]): the
/// file's own `tiers` array WAS the design, full stop. Restores a custom material
/// snapshot exactly like [`commit_loaded_native`] does, on the same
/// [`LoadNativeOnlyResult::restorable_custom_material`]/`material_resolution`
/// fields [`LoadPairedResult`] also carries.
fn open_native_self_contained(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
    load: SelfContainedLoad<'_>,
) {
    let SelfContainedLoad {
        native_path,
        loaded,
        asc_filename,
    } = load;
    let native_path_display = native_path.display().to_string();
    record_recent_native_file(ui, &native_path_display);
    // Same reasoning as `commit_loaded_native` -- this design's own
    // sidecar just landed here, so a later Save Native re-writing this exact path
    // needs no overwrite confirmation.
    CURRENT_NATIVE_PATH.with(|cell| *cell.borrow_mut() = Some(native_path.to_path_buf()));

    let (material_note, material_still_unresolved) = if let (Some(snapshot), Some(name)) = (
        loaded.restorable_custom_material.as_ref(),
        loaded.design.material.name.as_deref(),
    ) {
        let gem = gem_material_from_custom_snapshot(name, snapshot);
        let mut ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let materials = Arc::make_mut(&mut ctx.custom_materials);
        if let Some(pos) = materials
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name))
        {
            materials[pos] = gem;
        } else {
            materials.push(gem);
        }
        drop(ctx);
        (
            Some(format!(
                " '{name}' was restored from this file's own saved material data."
            )),
            false,
        )
    } else {
        let unresolved = matches!(
            loaded.material_resolution,
            indicatrix_cut_core::native::MaterialResolution::Unresolved
        );
        (
            unresolved.then(|| format!(" {}", loaded.material_resolution)),
            unresolved,
        )
    };
    let newer_version_note = loaded.written_by_newer_version.then(|| {
        " This file was written by a newer version of Indicatrix Cut; some settings \
          may not have been understood and could be lost on your next save."
            .to_string()
    });
    let is_mismatch = material_still_unresolved || loaded.written_by_newer_version;

    let printed_proportions = loaded.printed_proportions;
    state.borrow_mut().replace_wholesale(EditorState {
        deep_solve_result_generation: None,
        design: loaded.design,
        history: History::new(),
        printed_proportions,
        generation: Arc::new(AtomicU64::new(0)),
        design_epoch: Arc::new(AtomicU64::new(0)),
        saved_generation: 0,
        pending_unsaved_action: None,
        deep_solve: None,
        optimize: None,
        pending_optimize: Arc::new(Mutex::new(None)),
        // No paired `.asc` text exists at all -- see this function's own doc
        // comment; a later Save/Export builds a fresh one from scratch, exactly
        // like a brand-new design.
        asc_filename: Some(asc_filename.to_string()),
        original_asc_text: None,
        pending_gear_remap: None,
        pending_retarget: None,
        multi_selected: std::collections::BTreeSet::new(),
        last_pushed_scratch: RefCell::new(PushedScratch::default()),
        material_combo_cache: RefCell::new(MaterialComboCache::default()),
        source_entry_id: None,
        used_placeholder: false,
    });
    finish_state_replace(ui, render_ctx, preview_state, solid_last_solved, state);
    show_toast(
        ui,
        &format!(
            "Recovered '{native_path_display}' -- no paired .asc file was found, so this design \
             was rebuilt directly from the native file's own saved data.{}{}",
            material_note.unwrap_or_default(),
            newer_version_note.unwrap_or_default()
        ),
        if is_mismatch { "warning" } else { "success" },
    );
}

/// The bare-`.asc`-with-no-sidecar path -- builds the design exactly the
/// way `gui::editor::loading::design_from_asc_text` already does for a catalogue
/// attachment (no meet-intent overlay, no fingerprint, no draft flag: there is no
/// native file at all), then replaces `state` wholesale via [`finish_state_replace`],
/// the same tail [`commit_loaded_native`] runs for the paired case.
fn open_plain_asc(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
    asc_path: &Path,
    asc_text: &str,
) {
    let file_name = asc_path.file_name().map_or_else(
        || "design.asc".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    match super::loading::design_from_asc_text(&file_name, asc_text, None) {
        Ok(loaded) => {
            // A bare `.asc` has no native sidecar at all -- see `CURRENT_NATIVE_PATH`'s
            // own doc comment. Cleared rather than left at whatever the PREVIOUS
            // design's own save/open set it to, so a later Save Native here is never
            // mistaken for "re-saving that unrelated design's own file."
            CURRENT_NATIVE_PATH.with(|cell| *cell.borrow_mut() = None);
            // `replace_wholesale`, not a plain `*state.borrow_mut() = ...`: carries
            // this state's own `generation` `Arc` across the replacement (and bumps
            // it) instead of handing back a brand-new one, so a background Deep
            // Solve/Optimize/auto-solve dispatched against the design being replaced
            // still observes the change -- see that method's own doc comment
            // (`state/mod.rs`) and `tier_actions::do_new_design_create`'s matching
            // comment for the same reasoning applied to New/Load Selected.
            state.borrow_mut().replace_wholesale(EditorState {
                design: loaded.design,
                history: History::new(),
                printed_proportions: None,
                generation: Arc::new(AtomicU64::new(0)),
                design_epoch: Arc::new(AtomicU64::new(0)),
                saved_generation: 0,
                pending_unsaved_action: None,
                deep_solve: None,
                optimize: None,
                pending_optimize: Arc::new(Mutex::new(None)),
                deep_solve_result_generation: None,
                asc_filename: loaded.asc_filename,
                original_asc_text: loaded.original_asc_text,
                pending_gear_remap: None,
                pending_retarget: None,
                multi_selected: std::collections::BTreeSet::new(),
                last_pushed_scratch: RefCell::new(PushedScratch::default()),
                material_combo_cache: RefCell::new(MaterialComboCache::default()),
                // Open Native (this path and the paired-load
                // one below) has no catalogue row of its own -- it loaded from a
                // file the cutter picked directly, not from a library selection --
                // so there is nothing here for a later Save Native to write back
                // to. `gui::editor::callbacks::tier_actions::setup_load_selected_callback`'s
                // local branch is the one place this is ever `Some`.
                source_entry_id: None,
                // A native/plain-`.asc` open carries a real recorded
                // schedule, never the angle-table reconstruction fallback.
                used_placeholder: false,
            });
            finish_state_replace(ui, render_ctx, preview_state, solid_last_solved, state);
            show_toast(
                ui,
                &format!(
                    "Loaded '{}' (plain .asc, no native sidecar found -- authored meet \
                     constraints and detached facets are not available).",
                    asc_path.display()
                ),
                "success",
            );
        }
        Err(e) => show_toast(ui, &format!("Cannot open: {e}"), "error"),
    }
}

/// The tail every "replace `EditorState` wholesale" open path shares, once the new
/// state is already stored: refresh the viewport/panel from it, then reset selection
/// unconditionally (a previously selected tier index now names, at best, an
/// unrelated row in whatever design just replaced it) and mark the fresh state clean.
/// Shared by [`commit_loaded_native`] (a native+`.asc` pair) and [`open_plain_asc`] (a
/// bare `.asc`) so the "reset selection, mark clean" sequence is written exactly once
/// for both -- the same reset `gui::editor::callbacks::tier_actions::apply_loaded_design`/
/// `setup_new_design_create_callback` run for Load Selected/New.
fn finish_state_replace(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
    state: &Rc<RefCell<EditorState>>,
) {
    // A Deep Solve or Optimize verdict describes the design that was just replaced,
    // so it must not outlive it -- see `clear_analysis_results`' own doc comment.
    clear_analysis_results(ui);
    let st = state.borrow();
    refresh_all(ui, render_ctx, preview_state, solid_last_solved, &st, true);
    // Names the window title (`MainWindow.loaded_design_name`)
    // after whatever design this replace just loaded -- covers both `open_native_pair`
    // and `open_plain_asc`, the two callers of this shared tail. Falls back to empty
    // (bare "Indicatrix Cut") only in the defensive case where `asc_filename` was
    // somehow never set, which neither caller actually does.
    ui.set_loaded_design_name(st.asc_filename.clone().unwrap_or_default().into());
    drop(st);
    ui.global::<EditorModel>().set_selected_tier_index(-1);
    let pulse = ui.global::<EditorModel>().get_form_reset_pulse();
    ui.global::<EditorModel>()
        .set_form_reset_pulse(pulse.wrapping_add(1));
    // Explicit rather than left to `EditorModel.recompute_dirty`'s reactive `changed
    // tiers` hook alone (`refresh_all` above does reassign `tiers`, so that hook would
    // catch this too) -- a freshly replaced `EditorState` is clean by construction
    // (`saved_generation`/`generation` both start at `0`), and saying so directly
    // here is one line, easier to verify than tracing through the Slint side.
    ui.global::<EditorModel>().set_is_dirty(false);
}

/// Bundles [`commit_loaded_native`]'s per-call payload -- kept as one struct (rather
/// than five more parameters) purely to keep that function under clippy's
/// argument-count lint, the same reasoning `tier_actions::LoadedDesignOutcome` uses.
struct LoadedNativeOutcome {
    loaded: LoadPairedResult,
    /// What the toast calls the file -- the picked native file's own display path.
    native_path_display: String,
    asc_filename: String,
    asc_text: String,
    /// Drives the toast's class: `"warning"` (which `gui::show_toast` never
    /// auto-dismisses, same as `"error"`, but is coloured and captioned as a note
    /// rather than a failure) rather than `"info"`'s 3.5-second flash,
    /// since a mismatch means something about this design's authored intent may not
    /// have made the round trip -- worth a permanent, plainly-worded note, not a
    /// flash a cutter can miss mid-click, and not styled as an error when nothing
    /// actually failed.
    is_mismatch: bool,
}

/// Replaces `state` wholesale with `outcome.loaded`'s design and pushes the result
/// into the panel/viewport -- the shared tail [`do_open_native`]'s clean path and both
/// of [`setup_mismatch_dialog_callbacks`]'s committing branches (Apply Anyway/Use .asc
/// Only) all funnel through, so the "replace state, reset selection, report the
/// outcome" sequence is written exactly once.
fn commit_loaded_native(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
    outcome: LoadedNativeOutcome,
) {
    let LoadedNativeOutcome {
        loaded,
        native_path_display,
        asc_filename,
        asc_text,
        is_mismatch,
    } = outcome;
    record_recent_native_file(ui, &native_path_display);
    // This design's OWN sidecar just landed here -- see
    // `CURRENT_NATIVE_PATH`'s own doc comment for why a later Save Native re-writing
    // this exact path needs no overwrite confirmation.
    CURRENT_NATIVE_PATH.with(|cell| *cell.borrow_mut() = Some(PathBuf::from(&native_path_display)));
    // A plain sentence, not
    // `FingerprintCheck`/`TierOverlay`'s own technical `Display` text (e.g.
    // "per-tier meet-intent overlay skipped (fingerprint mismatch)") -- see
    // `plain_load_outcome_text`'s own doc comment.
    let outcome_note = plain_load_outcome_text(&loaded.fingerprint, &loaded.tier_overlay);
    // A material name this build can't resolve AND whose sidecar carried a
    // `[material.custom]` snapshot is
    // restored into this session's own custom-material registry right now --
    // `RenderContext::custom_materials`, the same list the material editor's own
    // "Save Custom Material" pushes into (`gui::optics::custom_materials`) --
    // instead of silently rendering as Diamond. See
    // `LoadPairedResult::restorable_custom_material`'s own doc comment for why
    // this is the caller's job, not `load_paired`'s.
    // `material_still_unresolved` is `false` for a successful restoration -- that
    // is good news, not a mismatch, so it must not push this toast into the
    // persistent "warning" class below the way a genuinely unresolved material
    // does.
    let (material_note, material_still_unresolved) = if let (Some(snapshot), Some(name)) = (
        loaded.restorable_custom_material.as_ref(),
        loaded.design.material.name.as_deref(),
    ) {
        let gem = gem_material_from_custom_snapshot(name, snapshot);
        let mut ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let materials = Arc::make_mut(&mut ctx.custom_materials);
        if let Some(pos) = materials
            .iter()
            .position(|m| m.name.eq_ignore_ascii_case(name))
        {
            materials[pos] = gem;
        } else {
            materials.push(gem);
        }
        drop(ctx);
        (
            Some(format!(
                " '{name}' was restored from this file's own saved material data."
            )),
            false,
        )
    } else {
        // A material name this build can't resolve AND has no snapshot to
        // restore from (a sidecar with no saved material snapshot, or a
        // material that was never actually custom) still silently becomes
        // Diamond once `MaterialSelection::resolve` runs -- see
        // `MaterialResolution`'s own doc comment. Surfaced here rather than
        // swallowed, so at least the open toast says so.
        let unresolved = matches!(
            loaded.material_resolution,
            indicatrix_cut_core::native::MaterialResolution::Unresolved
        );
        (
            unresolved.then(|| format!(" {}", loaded.material_resolution)),
            unresolved,
        )
    };
    // `format_version` was written but never checked -- a sidecar from a
    // newer build could carry fields this one silently drops into `unknown` and
    // re-serializes on the next save (quietly degrading it further each round trip).
    // Warned rather than refused: every named field here already tolerates being
    // absent, so the design itself still loaded fine.
    let newer_version_note = loaded.written_by_newer_version.then(|| {
        " This file was written by a newer version of Indicatrix Cut; some settings \
          may not have been understood and could be lost on your next save."
            .to_string()
    });
    let is_mismatch = is_mismatch || material_still_unresolved || loaded.written_by_newer_version;

    // `replace_wholesale`, not a plain `*state.borrow_mut() = ...` -- see
    // `open_plain_asc`'s matching comment and `EditorState::replace_wholesale`'s own
    // doc comment (`state/mod.rs`) for why: this carries the OLD `generation` `Arc`
    // (and bumps it) across the replacement so a background Deep Solve/Optimize/
    // auto-solve dispatched against the design being replaced still observes the
    // change instead of comparing against a counter nobody increments anymore.
    // Restored from the sidecar's own `[source]` table (written by an
    // earlier Save Native -- see `EditorState::printed_proportions`'s own doc
    // comment) rather than hard-coded `None`, so Deep Solve still has printed figures
    // to verify against after a Save Native/Open Native round trip, not only on this
    // design's very first "Load Selected" from the catalogue. Still `None` for a
    // sidecar saved before printed proportions were recorded there, or one for a
    // design never loaded from a catalogue row at all.
    let printed_proportions = loaded.printed_proportions;
    state.borrow_mut().replace_wholesale(EditorState {
        deep_solve_result_generation: None,
        design: loaded.design,
        history: History::new(),
        printed_proportions,
        generation: Arc::new(AtomicU64::new(0)),
        design_epoch: Arc::new(AtomicU64::new(0)),
        saved_generation: 0,
        pending_unsaved_action: None,
        deep_solve: None,
        optimize: None,
        pending_optimize: Arc::new(Mutex::new(None)),
        asc_filename: Some(asc_filename),
        original_asc_text: Some(asc_text),
        pending_gear_remap: None,
        pending_retarget: None,
        multi_selected: std::collections::BTreeSet::new(),
        last_pushed_scratch: RefCell::new(PushedScratch::default()),
        material_combo_cache: RefCell::new(MaterialComboCache::default()),
        // Same reasoning as the plain-`.asc` Open Native path
        // above -- the native sidecar's own `[source]` table (`printed_proportions`,
        // just above) carries a catalogue row's PRINTED proportions, but not which
        // row it was loaded from, so there is nothing here to write back to either.
        source_entry_id: None,
        // A native/plain-`.asc` open carries a real recorded
        // schedule, never the angle-table reconstruction fallback.
        used_placeholder: false,
    });
    finish_state_replace(ui, render_ctx, preview_state, solid_last_solved, state);
    show_toast(
        ui,
        &format!(
            "Loaded '{native_path_display}'. {outcome_note}{}{}",
            material_note.unwrap_or_default(),
            newer_version_note.unwrap_or_default()
        ),
        if is_mismatch { "warning" } else { "success" },
    );
}

/// Plain-English replacement for [`FingerprintCheck`]/[`TierOverlay`]'s own
/// `Display` text in the load-outcome toast. The crate's internal diagnostic prose
/// ("per-tier meet-intent overlay skipped (fingerprint mismatch)") says nothing to
/// a cutter about what actually happened to their file, so this rewrites it in
/// plain language a cutter can read without knowing what a fingerprint or a tier
/// overlay is. Persistence and severity (a "warning" toast that stays until
/// dismissed, rather than a 3.5-second "info" flash) are handled separately by
/// the caller.
fn plain_load_outcome_text(fingerprint: &FingerprintCheck, tier_overlay: &TierOverlay) -> String {
    if matches!(fingerprint, FingerprintCheck::Match) {
        return match tier_overlay {
            TierOverlay::Applied | TierOverlay::AppliedDespiteMismatch => {
                "Your saved meet constraints were restored.".to_string()
            }
            TierOverlay::SkippedTierCountMismatch {
                native_tiers,
                asc_tiers,
            } => format!(
                "This file's saved tier count ({native_tiers}) does not match the .asc file's \
                 ({asc_tiers}), so your saved meet constraints could not be restored."
            ),
            TierOverlay::SkippedFingerprintMismatch => {
                // Not reached in practice once `fingerprint` is `Match` -- kept as its
                // own honest arm rather than assumed unreachable, since the two checks
                // are independent types with no shared invariant enforcing this.
                "Your saved meet constraints were restored.".to_string()
            }
            TierOverlay::AppliedFromDraft => "Loaded from an unsolved draft: the masts shown are \
                 placeholders, not a real solve."
                .to_string(),
        };
    }
    match tier_overlay {
        TierOverlay::AppliedDespiteMismatch => "The .asc file changed since this sidecar was \
             saved, so its geometry was used as-is; your saved meet constraints were re-applied \
             anyway, at your request, and may no longer line up with the changed tiers."
            .to_string(),
        _ => "The .asc file changed since this sidecar was saved, so its geometry was used as-is \
             and your saved meet constraints were not restored."
            .to_string(),
    }
}

/// The fingerprint-mismatch dialog's three resolution callbacks -- see
/// [`PendingMismatch`]/[`PENDING_MISMATCH`]. Registered once, from
/// [`setup_open_native_callback`], since only Open Native can ever populate
/// [`PENDING_MISMATCH`] in the first place.
fn setup_mismatch_dialog_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &super::view::SolidLastSolved,
) {
    let state_apply = Rc::clone(state);
    let render_ctx_apply = Arc::clone(render_ctx);
    let preview_state_apply = Arc::clone(preview_state);
    let solid_last_solved_apply = Arc::clone(solid_last_solved);
    let ui_weak_apply = ui.as_weak();
    ui.global::<EditorModel>()
        .on_mismatch_dialog_apply_anyway(move || {
            let Some(ui) = ui_weak_apply.upgrade() else {
                return;
            };
            ui.global::<EditorModel>().set_mismatch_dialog_open(false);
            let Some(pending) = PENDING_MISMATCH.with(RefCell::take) else {
                return;
            };
            // `true`: the cutter just explicitly asked for the sidecar's meets to
            // apply despite the mismatch -- see `TierOverlay::AppliedDespiteMismatch`.
            match load_paired(&pending.asc_text, &pending.native_text, true) {
                Ok(loaded) => commit_loaded_native(
                    &ui,
                    &state_apply,
                    &render_ctx_apply,
                    &preview_state_apply,
                    &solid_last_solved_apply,
                    LoadedNativeOutcome {
                        loaded,
                        native_path_display: pending.native_path_display,
                        asc_filename: pending.asc_filename,
                        asc_text: pending.asc_text,
                        is_mismatch: true,
                    },
                ),
                Err(e) => show_toast(&ui, &format!("Cannot open: {e}"), "error"),
            }
        });

    let state_asc_only = Rc::clone(state);
    let render_ctx_asc_only = Arc::clone(render_ctx);
    let preview_state_asc_only = Arc::clone(preview_state);
    let solid_last_solved_asc_only = Arc::clone(solid_last_solved);
    let ui_weak_asc_only = ui.as_weak();
    ui.global::<EditorModel>()
        .on_mismatch_dialog_asc_only(move || {
            let Some(ui) = ui_weak_asc_only.upgrade() else {
                return;
            };
            ui.global::<EditorModel>().set_mismatch_dialog_open(false);
            let Some(pending) = PENDING_MISMATCH.with(RefCell::take) else {
                return;
            };
            // `false` again: the cutter chose to keep the mismatch's overlay SKIPPED,
            // i.e. `TierOverlay::SkippedFingerprintMismatch` -- every authored meet
            // constraint and detached facet the sidecar carried is dropped, kept only
            // to whatever the plain `.asc` itself encodes.
            match load_paired(&pending.asc_text, &pending.native_text, false) {
                Ok(loaded) => commit_loaded_native(
                    &ui,
                    &state_asc_only,
                    &render_ctx_asc_only,
                    &preview_state_asc_only,
                    &solid_last_solved_asc_only,
                    LoadedNativeOutcome {
                        loaded,
                        native_path_display: pending.native_path_display,
                        asc_filename: pending.asc_filename,
                        asc_text: pending.asc_text,
                        is_mismatch: true,
                    },
                ),
                Err(e) => show_toast(&ui, &format!("Cannot open: {e}"), "error"),
            }
        });

    ui.global::<EditorModel>()
        .on_mismatch_dialog_cancel(move || {
            PENDING_MISMATCH.with(|cell| *cell.borrow_mut() = None);
        });
}

#[cfg(test)]
mod tests {
    use super::{NOT_CLOSED_SOLID_MARKER, degenerate_marker_header, plain_load_outcome_text};
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{
        ConstraintTier, Design, FingerprintCheck, FreshDesignSpec, MaterialSelection, PreformSpec,
        TierOverlay, load_paired, save_paired,
    };

    /// Material name/RI override, gear, symmetry and mirror all round-trip through
    /// the exact pair of functions
    /// [`setup_save_native_callback`]/[`setup_open_native_callback`] call
    /// (`indicatrix_cut_core::save_paired`/`load_paired`) -- verified here directly rather
    /// than trusted, since this crate's own wiring exercises gear/symmetry/mirror
    /// persistence only through the editor's own "New Design"/design-settings forms,
    /// not through a dedicated round-trip test. `gear`/`symmetry`/
    /// `mirror` round-trip through the paired `.asc`'s own header (already
    /// exercised, indirectly, by every existing "Open Native" test in
    /// `indicatrix_cut_core::native`); `material`/`refractive_index_override` round-trip
    /// through the native sidecar's `[material]` table (already
    /// unit-tested in `indicatrix_cut_core::native` directly) -- this test's own
    /// value is confirming the ONE combination this app actually writes (a
    /// design with all four set together, via the same `save_paired`/
    /// `load_paired` this module's own callbacks call) survives intact.
    #[test]
    fn gear_symmetry_mirror_and_material_all_round_trip_through_save_and_open() {
        let spec = FreshDesignSpec {
            gear_teeth: 80,
            symmetry_order: 5,
            mirror: false,
            material: MaterialSelection {
                name: Some("Quartz".to_string()),
                specific_gravity_override: Some(2.65),
                refractive_index_override: Some(1.55),
            },
            preform: PreformSpec::cylinder(80, 1.4, 1.0, 1.3),
        };
        let mut design = Design::fresh_from_spec(spec);
        // A schedule with zero tiers exports (and re-solves) fine, but
        // `indicatrix_formats::asc::parse_asc` refuses to parse an `.asc` with no
        // facet ('a') records at all -- one real, anchored tier is what a
        // saved design would actually look like.
        design.tiers.push(ConstraintTier {
            angle_deg: -40.0,
            name: "P1".to_string(),
            indices: vec![0.0, 16.0, 32.0, 48.0, 64.0],
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });

        let saved = save_paired(&design, "roundtrip.asc", None, None, None)
            .expect("a fresh design must save");
        let loaded =
            load_paired(&saved.asc_text, &saved.native_toml, false).expect("must load back");

        assert_eq!(loaded.design.meta.gear_teeth, 80);
        assert_eq!(loaded.design.meta.symmetry_order, 5);
        assert!(!loaded.design.meta.mirror);
        assert_eq!(loaded.design.material.name.as_deref(), Some("Quartz"));
        assert_eq!(loaded.design.material.specific_gravity_override, Some(2.65));
        assert_eq!(loaded.design.material.refractive_index_override, Some(1.55));
        // The effective RI actually written to `.asc`'s `I` line -- confirms
        // the override, not just the raw field, made the round trip in a way
        // that would show up in the exported schedule too.
        assert!((loaded.design.effective_refractive_index() - 1.55).abs() < 1e-9);
    }

    // --- degenerate_marker_header ---

    #[test]
    fn degenerate_marker_header_stamps_the_reason_when_absent() {
        let headers: Vec<String> = vec!["GemCad 5.0".to_string()];
        let header = degenerate_marker_header(&headers, "Degenerate: only 2 distinct vertices.")
            .expect("no existing marker -- must stamp one");
        assert!(header.starts_with(NOT_CLOSED_SOLID_MARKER));
        assert!(header.contains("Degenerate: only 2 distinct vertices."));
    }

    #[test]
    fn degenerate_marker_header_never_stamps_twice() {
        let headers = vec![format!("{NOT_CLOSED_SOLID_MARKER} -- already noted")];
        assert!(degenerate_marker_header(&headers, "a different message").is_none());
    }

    // --- plain_load_outcome_text ---

    #[test]
    fn a_clean_match_and_applied_overlay_reads_as_restored() {
        let text = plain_load_outcome_text(&FingerprintCheck::Match, &TierOverlay::Applied);
        assert_eq!(text, "Your saved meet constraints were restored.");
    }

    #[test]
    fn a_tier_count_mismatch_names_both_counts_even_on_a_clean_fingerprint() {
        let text = plain_load_outcome_text(
            &FingerprintCheck::Match,
            &TierOverlay::SkippedTierCountMismatch {
                native_tiers: 5,
                asc_tiers: 6,
            },
        );
        assert!(text.contains('5') && text.contains('6'), "{text}");
        assert!(
            !text.contains("fingerprint"),
            "must read in plain English, not the crate's own diagnostic vocabulary: {text}"
        );
    }

    #[test]
    fn a_fingerprint_mismatch_says_the_asc_changed_and_constraints_were_not_restored() {
        let text = plain_load_outcome_text(
            &FingerprintCheck::Mismatch {
                expected_sha256: "aaaa".to_string(),
                found_sha256: "bbbb".to_string(),
            },
            &TierOverlay::SkippedFingerprintMismatch,
        );
        assert!(
            text.contains("changed since this sidecar was saved"),
            "{text}"
        );
        assert!(text.contains("not restored"), "{text}");
        assert!(
            !text.contains("sha256"),
            "must not leak the technical hash text: {text}"
        );
    }

    #[test]
    fn applying_despite_a_mismatch_says_it_was_at_the_cutters_own_request() {
        let text = plain_load_outcome_text(
            &FingerprintCheck::Mismatch {
                expected_sha256: "aaaa".to_string(),
                found_sha256: "bbbb".to_string(),
            },
            &TierOverlay::AppliedDespiteMismatch,
        );
        assert!(text.contains("at your request"), "{text}");
    }

    #[test]
    fn a_draft_overlay_on_a_clean_match_names_the_placeholder_masts() {
        let text =
            plain_load_outcome_text(&FingerprintCheck::Match, &TierOverlay::AppliedFromDraft);
        assert!(text.contains("placeholders"), "{text}");
    }

    // --- Group 1, cached-solve reuse ---

    use super::{
        StatusDecision, decide_write_status, solve_matches_design, temp_sibling, write_autosave,
    };

    /// The same round-brilliant fixture `cut_sheet.rs`'s own tests use: 8 tiers,
    /// all `ScaleReference`, always solves and closes.
    fn round_brilliant_design() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            indicatrix_cut_core::ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    #[test]
    fn solve_matches_design_accepts_a_cache_whose_tier_count_still_matches() {
        let design = round_brilliant_design();
        let solved = design.solve().expect("standard round brilliant solves");
        assert_eq!(solved.len(), design.tiers.len());
        assert!(solve_matches_design(Some(&solved), &design).is_some());
    }

    #[test]
    fn solve_matches_design_rejects_a_cache_whose_tier_count_no_longer_matches() {
        let design = round_brilliant_design();
        let solved = design.solve().expect("standard round brilliant solves");
        // One tier short of `design.tiers.len()` -- the shape an edit that added or
        // removed a tier since this solve was cached would leave behind.
        assert!(solve_matches_design(Some(&solved[..solved.len() - 1]), &design).is_none());
    }

    #[test]
    fn solve_matches_design_rejects_no_cache_at_all() {
        let design = round_brilliant_design();
        assert!(solve_matches_design(None, &design).is_none());
    }

    // --- Group 2, the write-confirm decision ---

    #[test]
    fn decide_write_status_is_fine_for_a_closed_solid() {
        let design = round_brilliant_design();
        let solved = design.solve().expect("standard round brilliant solves");
        assert!(matches!(
            decide_write_status(&design, Ok(solved.as_slice())),
            StatusDecision::Fine
        ));
    }

    #[test]
    fn decide_write_status_needs_confirm_when_the_design_does_not_solve() {
        let design = round_brilliant_design();
        match decide_write_status(&design, Err("no scale-reference tier")) {
            StatusDecision::NeedsConfirm(message) => {
                assert_eq!(message, "no scale-reference tier");
            }
            StatusDecision::Fine => panic!("a solve error must always need confirmation"),
        }
    }

    // The picker test hook's own take/set mechanics live in `gui::pickers`
    // along with the picker itself -- see that module's own test suite
    // (`pick_test_hook_is_consumed_exactly_once`) for the equivalent coverage.

    // --- Group 4, autosave write round trip ---

    #[test]
    fn write_autosave_round_trips_its_own_toml_text() {
        let path = std::env::temp_dir().join(format!(
            "indicatrix_cut_write_autosave_test_{}.toml",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        write_autosave(&path, "design = \"round trip\"\n").expect("write must succeed");
        let read_back = std::fs::read_to_string(&path).expect("must read back what was written");
        assert_eq!(read_back, "design = \"round trip\"\n");

        // The stage-then-rename discipline: no leftover `.tmp` sibling
        // once the write has completed.
        assert!(!temp_sibling(&path).is_file());

        let _ = std::fs::remove_file(&path);
    }
}
