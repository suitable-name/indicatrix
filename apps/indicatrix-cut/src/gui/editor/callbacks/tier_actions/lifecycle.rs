//! The design-lifecycle callbacks: Solve, New Design, Load Selected, and the
//! Save/Discard/Cancel unsaved-changes guard they share with Open.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, atomic::AtomicU64},
};

use indicatrix_cut_core::History;
use indicatrix_editor::{EditorSession, guide::solving_events::SOLVE_REQUESTED};
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;

use super::{misc::bump_form_reset_pulse, new_design::do_new_design_create};
use crate::{
    EditorModel, LibraryModel, MainWindow,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::{
        editor::{
            callbacks::solve_actions::clear_analysis_results,
            loading,
            material_lookup::nearest_built_in_material,
            native_io::{self, AfterSave, do_open_native},
            stall_guard::stall_guard,
            state::{
                ANGLE_NUDGE_COALESCE_WINDOW, DesignFileExtras, EditorState, MaterialComboCache,
                PendingUnsavedAction, PushedScratch,
            },
            view::{SolidLastSolved, push_has_design, refresh_all_now},
        },
        library::remote::fetch_remote_design_source,
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
        tutorial_events::raise,
    },
};

/// "Solve": the explicit re-solve action -- see this group's `mod.rs` doc comment for
/// why every other edit callback deliberately does NOT do this. The only callback
/// here besides `New`/`Load Selected` that calls `refresh_all` (a real `Design::solve`,
/// potentially multi-second) rather than [`refresh_editor_panel_stale`].
///
/// Guards against Deep Solve or Optimize already running, not only a
/// re-entrant Solve click. Solve, Deep Solve and Optimize each
/// guard only their OWN `*_running` flag, so any two of the three could be launched
/// at once (each roughly doubling the others' runtime). `EditorModel.busy_action`
/// (`ui/models/editor.slint`) is the single Slint-side derived source of truth for
/// all three, shown in `EditorCommandBar`/`EditorStatusStrip`; this reads the three
/// flags it is built from directly (equivalent to checking `busy_action != ""`)
/// since that is what every other `on_*` guard in this module already reads, and
/// pulling in a fourth (derived) getter here would buy nothing.
pub(in crate::gui::editor) fn setup_solve_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_solve(move || {
        stall_guard("on_solve", || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let model = ui.global::<EditorModel>();
            if model.get_solve_running()
                || model.get_deep_solve_running()
                || model.get_optimize_running()
            {
                return;
            }
            refresh_all_now(
                &ui,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &state,
                false,
            );
            // The Solve button was pressed and the solve has landed (the tutorials' event).
            raise(&ui, SOLVE_REQUESTED);
        });
    });
}

thread_local! {
    /// The live `EditorState`, stashed here purely so
    /// [`setup_load_selected_callback`]'s remote branch can still reach it once
    /// control is back on the UI thread. `gui::library::remote::fetch_remote_design_source`
    /// (itself wrapping `bridge::library::source::spawn_library_request`) requires its
    /// completion callback to be `Send + 'static`, because that callback's value is
    /// physically moved through a `std::thread::spawn` closure before being handed to
    /// `Weak::upgrade_in_event_loop` -- and an `Rc<RefCell<EditorState>>` capture can
    /// never satisfy `Send` (Rc's ref-count is non-atomic), no matter that the
    /// callback only ever actually RUNS back on the UI thread. Mirrors
    /// `auto_solve::RUNTIME`'s own `thread_local!` for the identical reason (see that
    /// module's doc comment). Set once, the first time [`setup_load_selected_callback`]
    /// runs -- there is only ever one `EditorState` for the app's lifetime -- and read
    /// back (never cleared) inside the remote branch's completion closure, which never
    /// captures the `Rc` itself.
    static REMOTE_LOAD_TARGET: RefCell<Option<Rc<RefCell<EditorState>>>> = const { RefCell::new(None) };
}

/// Bundles [`apply_loaded_design`]'s per-call payload -- kept as one struct (rather
/// than three more parameters) purely to keep that function under clippy's
/// argument-count lint, the same reasoning `auto_solve::BackgroundSolveResult` uses.
struct LoadedDesignOutcome<'a> {
    loaded: loading::LoadedDesign,
    printed_proportions: Option<indicatrix::geometry::stone_metrics::ExternalProportions>,
    /// What the success toast calls the design -- see [`apply_loaded_design`]'s own
    /// doc comment.
    label: &'a str,
    /// The catalogue row this design was loaded from -- `Some(full.entry_id)` for a
    /// LOCAL load, `None` for a remote one (see `EditorState::source_entry_id`'s own
    /// doc comment for why a remote entry id must never land here). Threaded through
    /// unconditionally rather than defaulted to `None` here: every
    /// `LoadedDesignOutcome` construction site, including `native_io.rs`'s own, must
    /// supply the correct value for this field rather than rely on a default.
    source_entry_id: Option<i64>,
    /// The catalogue entry's `url` for a LOCAL load, `None` for a remote one. A catalogue
    /// design whose attached file has no `[meta].id` is named by the UUID derived from
    /// this `url`, so reopening the same entry finds its saved variants again (see
    /// `state::design_identity`). A remote design gets a fresh UUID instead: its `.asc`
    /// file name says nothing stable about which design it is.
    catalogue_url: Option<&'a str>,
}

/// The shared tail of [`setup_load_selected_callback`]'s local and remote branches:
/// replaces `state` wholesale with `loaded`'s design (fresh `History`, every other
/// per-design transient field cleared -- the same wholesale replacement
/// `setup_new_design_create_callback` does for "New"), pushes a real solve into the
/// panel/viewport via `refresh_all`, shows the "loaded"/"placeholder" toast, and
/// offers the catalogue material-suggestion banner built from `loaded.design`'s own
/// schedule RI.
///
/// `loaded_label` names what the success toast calls the design: the local branch's
/// own catalogue title, or the remote branch's bare `.asc` file name -- a remote
/// fetch (`gui::library::remote::RemoteDesignSource`) never carries the catalogue
/// title alongside its `.asc` text, only the attachment's own name.
fn apply_loaded_design(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    outcome: LoadedDesignOutcome<'_>,
) {
    let LoadedDesignOutcome {
        loaded,
        printed_proportions,
        label: loaded_label,
        source_entry_id,
        catalogue_url,
    } = outcome;
    let schedule_ri = loaded.design.meta.refractive_index;
    let mut st = state.borrow_mut();
    // `replace_wholesale`, not a plain `*st = ...` -- see `do_new_design_create`'s
    // matching comment and `EditorState::replace_wholesale`'s own doc comment: this
    // carries the OLD `generation` `Arc` (and bumps it) across the replacement so a
    // background Deep Solve/Optimize/auto-solve dispatched against the design being
    // replaced still observes the change instead of comparing against a counter
    // nobody increments anymore.
    st.replace_wholesale(EditorState {
        // Read before `loaded.design` moves below -- a bare bool/i64 field read
        // never conflicts with a later partial move of a DIFFERENT field of the
        // same `loaded`/`outcome`, but keeping the "used for something else"
        // fields together here (rather than scattered) is easier to audit.
        used_placeholder: loaded.used_placeholder,
        // The design file's `[meta]` and attachments (empty for a bare `.asc`), kept so
        // the next Save writes them back unchanged.
        file_extras: DesignFileExtras::new(loaded.metadata, loaded.attachments)
            .with_design_uuid_assigned(catalogue_url),
        source_entry_id,
        // See `EditorState::fresh`'s matching comment on the coalescing window.
        session: EditorSession::with_history(
            loaded.design,
            History::with_coalesce_window(ANGLE_NUDGE_COALESCE_WINDOW),
        ),
        printed_proportions,
        design_epoch: Arc::new(AtomicU64::new(0)),
        pending_unsaved_action: None,
        after_save: None,
        deep_solve: None,
        optimize: None,
        pending_optimize: Arc::new(Mutex::new(None)),
        deep_solve_result_generation: None,
        asc_filename: loaded.asc_filename,
        original_asc_text: loaded.original_asc_text,
        pending_gear_remap: None,
        pending_retarget: None,
        // A freshly replaced `EditorState` has never pushed anything yet -- matches
        // `EditorState::fresh_from_spec`'s own construction (`state/mod.rs`); every
        // other `EditorState` construction site, including `native_io.rs`'s own,
        // must set this field the same way.
        last_pushed_scratch: RefCell::new(PushedScratch::default()),
        // Same reasoning as `last_pushed_scratch` immediately above -- a freshly
        // replaced `EditorState` has no cached material-combo options yet either.
        material_combo_cache: RefCell::new(MaterialComboCache::default()),
        has_design: true,
    });
    // A Deep Solve/Optimize verdict computed against the design just replaced no
    // longer describes anything on screen -- see `clear_analysis_results`'s own
    // doc comment.
    clear_analysis_results(ui);
    // Captured before dropping the borrow below -- `refresh_all_now` needs to
    // borrow `state` itself, so `st` cannot still be held across that call.
    let loaded_asc_filename = st.asc_filename.clone();
    drop(st);
    refresh_all_now(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        state,
        true,
    );
    // The whole `EditorState` above was just replaced -- any previously selected
    // tier index now names (at best) an unrelated row in the NEWLY loaded design, so
    // both halves of the selection reset unconditionally: the property (for the
    // Solid/Diagram overlay tint) and the pulse (so `EditorView`'s form-reset watcher
    // fires even when the selection happened to already be `-1`; see
    // `EditorModel::form_reset_pulse`'s own doc comment).
    ui.global::<EditorModel>().set_selected_tier_index(-1);
    bump_form_reset_pulse(ui);
    // Explicit, same reasoning as `native_io::commit_loaded_native`'s own matching
    // line: a freshly replaced `EditorState` is clean by construction.
    ui.global::<EditorModel>().set_is_dirty(false);
    // A loaded design is a real design: the empty-state card grid gives way to it.
    push_has_design(ui, &state.borrow());
    // The window title names whichever design is open -- `native_io` sets this on
    // every Save/Open, and this is the matching Load Selected path.
    ui.set_loaded_design_name(loaded_asc_filename.unwrap_or_default().into());
    if loaded.used_placeholder {
        show_toast(
            ui,
            "Loaded a reconstructed schedule -- mast distances are \
             placeholders (no attached .asc file was found); adjust \
             masts before exporting.",
            // Every mast on this design is a fabricated 0.0. A cutter who
            // misses that can export a file that looks like a real cut
            // instruction and is not, so this must not auto-dismiss the way an
            // "info" toast does.
            "warning",
        );
    } else {
        show_toast(
            ui,
            &format!("Loaded '{loaded_label}' into the editor."),
            "success",
        );
    }
    // Suggests the built-in material whose n_D is nearest the schedule RI (within
    // 0.01) -- never applied automatically, only offered as a banner the user can
    // dismiss or accept.
    if let Some((name, ri)) = nearest_built_in_material(schedule_ri, 0.01) {
        // Unrelated pre-existing build break fixed in passing (not part of this
        // session's GPU/display-thread work): the suggestion text is built BEFORE
        // `name` moves into `set_material_suggestion_name` below, not after.
        let suggestion_text = format!("Set material to {name} (RI {ri:.4})?");
        ui.global::<EditorModel>()
            .set_material_suggestion_name(name.into());
        ui.global::<EditorModel>()
            .set_material_suggestion_text(suggestion_text.into());
    } else {
        ui.global::<EditorModel>()
            .set_material_suggestion_name("".into());
        ui.global::<EditorModel>()
            .set_material_suggestion_text("".into());
    }
}

/// "Load Selected": replaces the editor state with the currently selected catalogue
/// design. Local library: reads the full record straight from `db` and prefers a
/// real attached `.asc` (see `loading::design_from_full_record`). Remote library:
/// fetches that entry's own `.asc` text off the UI thread
/// (`gui::library::remote::fetch_remote_design_source`) and builds the identical
/// `Design` from it via `loading::design_from_asc_text` -- there is no printed-
/// proportions/placeholder-reconstruction data available over that wire, so a
/// remote load never offers Deep Solve's external verification and never falls back
/// to the angle-table placeholder (a remote entry with no attached `.asc` reports
/// `DesignSourceNotAvailable`, surfaced as a toast instead).
///
/// Checks `entry_id` first (a plain, no-dialog-needed validation -- nothing
/// destructive would happen anyway), THEN [`EditorState::is_dirty`]: a dirty design
/// stashes [`PendingUnsavedAction::LoadSelected`] and opens the Save/Discard/Cancel
/// guard instead of replacing anything, resumed by [`setup_unsaved_guard_dispatch`]
/// (also wired up here -- see that function's own doc comment for why this is its
/// one call site) once "Save"/"Discard" is chosen. Everything past that point is
/// [`do_load_selected`], run either directly or as that resume.
pub(in crate::gui::editor) fn setup_load_selected_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let state = Rc::clone(state);
    REMOTE_LOAD_TARGET.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&state)));
    setup_unsaved_guard_dispatch(
        ui,
        &state,
        render_ctx,
        preview_state,
        solid_last_solved,
        db,
        source,
    );
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let db = Arc::clone(db);
    let source = Arc::clone(source);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_load_selected(move || {
        stall_guard("on_load_selected", || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
            if entry_id < 0 {
                show_toast(&ui, "No diagram selected to load.", "error");
                return;
            }

            if state.borrow().is_dirty() {
                state.borrow_mut().pending_unsaved_action =
                    Some(PendingUnsavedAction::LoadSelected);
                ui.global::<EditorModel>().set_unsaved_dialog_message(
                    "Loading another design will discard the current one's unsaved changes.".into(),
                );
                ui.global::<EditorModel>().set_unsaved_dialog_open(true);
                return;
            }
            do_load_selected(
                &ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &db,
                &source,
            );
        });
    });
}

/// Whether replacing the design now would throw away edits made since `captured`, the
/// generation the cutter last decided about: the generation moved AND the design is
/// unsaved. A generation that did not move means the cutter already answered for
/// exactly this design (a "Discard" that resumed the load leaves it dirty by choice).
const fn discards_edits_since(captured: u64, current: u64, is_dirty: bool) -> bool {
    current != captured && is_dirty
}

/// [`discards_edits_since`] against the live state.
fn edited_since(state: &Rc<RefCell<EditorState>>, captured: u64) -> bool {
    let st = state.borrow();
    discards_edits_since(captured, st.current_generation(), st.is_dirty())
}

/// Puts the Save/Discard/Cancel guard back up for a Load Selected whose remote
/// download finished after the design gained unsaved edits: nothing has been
/// replaced, and "Save"/"Discard" resume the load (a fresh download) exactly like
/// the first dirty check does.
fn ask_again_before_load(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    state.borrow_mut().pending_unsaved_action = Some(PendingUnsavedAction::LoadSelected);
    let model = ui.global::<EditorModel>();
    model.set_unsaved_dialog_message(
        "The design was edited while the remote design was downloading. Loading it will \
         discard those unsaved changes."
            .into(),
    );
    model.set_unsaved_dialog_open(true);
}

/// The actual "Load Selected" work -- see [`setup_load_selected_callback`]'s own doc
/// comment for why the dirty check runs before this is ever called, not inside it.
/// `entry_id` is re-read from [`LibraryModel::get_selected_entry_id`] here rather than
/// threaded through as a parameter: the guard dialog blocks every other interaction
/// while it is up, so the selection cannot have changed by the time this runs, and
/// re-reading it is one line versus a parameter every other caller would also need.
fn do_load_selected(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let entry_id = ui.global::<LibraryModel>().get_selected_entry_id();
    if entry_id < 0 {
        show_toast(ui, "No diagram selected to load.", "error");
        return;
    }

    let current_source = source
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    if let Some(worker) = current_source.worker().cloned() {
        // Remote: fetch this entry's own `.asc` text off the UI thread. The
        // completion closure below must be `Send + 'static` (see
        // `REMOTE_LOAD_TARGET`'s own doc comment for why it deliberately never
        // captures `state` directly) -- everything it DOES capture
        // (`render_ctx`/`preview_state`/`solid_last_solved`) is already `Arc`-
        // wrapped Send-safe data this crate moves across thread boundaries
        // elsewhere (e.g. `auto_solve::dispatch_background_solve`).
        let render_ctx = Arc::clone(render_ctx);
        let preview_state = Arc::clone(preview_state);
        let solid_last_solved = Arc::clone(solid_last_solved);
        // The download is asynchronous and the editor stays interactive meanwhile:
        // the generation as of the dirty check (or the Save/Discard answer) is
        // compared again when the design arrives, before anything is replaced.
        let requested_generation = state.borrow().current_generation();
        fetch_remote_design_source(
            ui.as_weak(),
            worker,
            i64::from(entry_id),
            move |ui, result| match result {
                Ok(remote) => {
                    match loading::design_from_asc_text(&remote.file_name, &remote.asc_text, None) {
                        Ok(loaded) => REMOTE_LOAD_TARGET.with(|cell| {
                            let target = cell.borrow().clone();
                            if let Some(state) = target {
                                if edited_since(&state, requested_generation) {
                                    ask_again_before_load(ui, &state);
                                    return;
                                }
                                apply_loaded_design(
                                    ui,
                                    &state,
                                    &render_ctx,
                                    &preview_state,
                                    &solid_last_solved,
                                    LoadedDesignOutcome {
                                        loaded,
                                        printed_proportions: None,
                                        label: &remote.file_name,
                                        // A remote entry id and a local row id occupy
                                        // independent id spaces (`EditorState::
                                        // source_entry_id`'s own doc comment) -- never
                                        // stored here.
                                        source_entry_id: None,
                                        catalogue_url: None,
                                    },
                                );
                            }
                        }),
                        Err(e) => show_toast(
                            ui,
                            &format!(
                                "'{}' failed to parse as a .asc cutting instructions: {e}",
                                remote.file_name
                            ),
                            "error",
                        ),
                    }
                }
                Err(e) => show_toast(ui, &e, "error"),
            },
        );
        return;
    }

    // Local: read the full record straight from the DB.
    let full = {
        let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        db.get_diagram_full(i64::from(entry_id))
    };
    let Ok(Some(full)) = full else {
        show_toast(ui, "Diagram detail not found.", "error");
        return;
    };
    match loading::design_from_full_record(&full) {
        Ok(loaded) => {
            let printed_proportions = loading::external_proportions_from_full_record(&full);
            apply_loaded_design(
                ui,
                state,
                render_ctx,
                preview_state,
                solid_last_solved,
                LoadedDesignOutcome {
                    loaded,
                    printed_proportions,
                    label: &full.title,
                    source_entry_id: Some(full.entry_id),
                    catalogue_url: Some(&full.url),
                },
            );
        }
        Err(e) => show_toast(ui, &e, "error"),
    }
}

/// The render/preview plumbing [`run_pending_unsaved_action`]'s three resumable
/// actions (New/Load Selected/Open) all need alongside `ui`/`state` --
/// bundled purely to keep that function's own argument count under clippy's
/// `too_many_arguments` lint; each field is unpacked back to its own `do_*`
/// parameter at the call, so no callee's signature changes.
#[derive(Clone, Copy)]
struct RenderPreviewHandles<'a> {
    render_ctx: &'a Arc<Mutex<RenderContext>>,
    preview_state: &'a Arc<SolidPreviewState>,
    solid_last_solved: &'a SolidLastSolved,
}

/// Runs whichever [`PendingUnsavedAction`] `pending` holds, or does nothing for
/// `None`. Shared by [`setup_unsaved_guard_dispatch`]'s "Discard" handler (which
/// takes it straight off `state` itself, immediately) and the [`AfterSave::Resume`]
/// listener registered in that same function (which takes it out of `after_save`
/// instead, once the "Save" resolution's own save actually lands -- see
/// [`AfterSave`]'s own doc comment) -- the only two ways to reach past the
/// Save/Discard/Cancel guard. Takes `pending` by value (already taken out of
/// wherever it lived) rather than reading `state.pending_unsaved_action` itself:
/// every arm below calls back into a `do_*` function that itself starts with its
/// own `state.borrow_mut()`, so this must never itself be holding that guard's
/// `RefMut` across the call.
fn run_pending_unsaved_action(
    pending: Option<PendingUnsavedAction>,
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    handles: &RenderPreviewHandles<'_>,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let RenderPreviewHandles {
        render_ctx,
        preview_state,
        solid_last_solved,
    } = *handles;
    match pending {
        Some(PendingUnsavedAction::New {
            spec,
            template_index,
        }) => {
            do_new_design_create(
                ui,
                state,
                render_ctx,
                preview_state,
                solid_last_solved,
                spec,
                template_index,
            );
        }
        Some(PendingUnsavedAction::LoadSelected) => {
            do_load_selected(
                ui,
                state,
                render_ctx,
                preview_state,
                solid_last_solved,
                db,
                source,
            );
        }
        Some(PendingUnsavedAction::OpenNative) => {
            do_open_native(ui, state, render_ctx, preview_state, solid_last_solved);
        }
        None => {}
    }
}

/// The Save/Discard/Cancel unsaved-changes guard's three resolution callbacks --
/// shared by New/Load Selected/Open (see [`PendingUnsavedAction`]), since only
/// one of them can ever be pending at a time and Slint only keeps the LAST handler
/// registered for a given callback. Registered once, from
/// [`setup_load_selected_callback`] -- the one owned function with every one of
/// `db`/`source` (needed to resume Load Selected) alongside the render/preview
/// plumbing New and Open also need, so it is the natural single home for this
/// rather than splitting it across the files that own each individual action.
///
/// "Save" invokes `EditorModel.save_native` (whatever handler is registered for it --
/// `native_io::setup_save_native_callback`, wired up independently of this function)
/// and only resumes the pending action once that save actually lands -- via the
/// [`AfterSave::Resume`] listener registered below, not a synchronous `is_dirty`
/// check run right after `invoke_save_native` returns: Save is
/// asynchronous end to end, so that check used to read the state from BEFORE the
/// save even started, silently dropping the pending action instead of ever resuming
/// it. A cancelled or failed save (already toasted by `save_native` itself, and
/// always clearing `after_save` -- see that enum's own doc comment) simply never
/// fires the listener, leaving the pending action untouched -- so it is dropped
/// here anyway, consistently with "Save" always ending the guard one way or another.
fn setup_unsaved_guard_dispatch(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    // Runs once the save this guard's own "Save" resolution triggered actually
    // completes -- see [`AfterSave::Resume`]'s own doc comment. A listener
    // that finds `after_save` holding the OTHER guard's own `CloseWindow` (or
    // `None`) must leave it untouched -- see `native_io::on_save_completed`'s own
    // doc comment; only one listener may ever actually consume a given value.
    let render_ctx_listener = Arc::clone(render_ctx);
    let preview_state_listener = Arc::clone(preview_state);
    let solid_last_solved_listener = Arc::clone(solid_last_solved);
    let db_listener = Arc::clone(db);
    let source_listener = Arc::clone(source);
    native_io::on_save_completed(move |ui, state| {
        let pending = {
            let mut st = state.borrow_mut();
            match st.after_save.take() {
                Some(AfterSave::Resume(action)) => Some(action),
                other => {
                    st.after_save = other;
                    return;
                }
            }
        };
        run_pending_unsaved_action(
            pending,
            ui,
            state,
            &RenderPreviewHandles {
                render_ctx: &render_ctx_listener,
                preview_state: &preview_state_listener,
                solid_last_solved: &solid_last_solved_listener,
            },
            &db_listener,
            &source_listener,
        );
    });

    let state_save = Rc::clone(state);
    let ui_weak_save = ui.as_weak();
    ui.global::<EditorModel>().on_unsaved_dialog_save(move || {
        let Some(ui) = ui_weak_save.upgrade() else {
            return;
        };
        ui.global::<EditorModel>().set_unsaved_dialog_open(false);
        let Some(action) = state_save.borrow_mut().pending_unsaved_action.take() else {
            return;
        };
        // Marks `after_save` so the listener above resumes `action` once this
        // save really lands -- see this function's own doc comment.
        state_save.borrow_mut().after_save = Some(AfterSave::Resume(action));
        ui.global::<EditorModel>().invoke_save_native();
    });

    let state_discard = Rc::clone(state);
    let render_ctx_discard = Arc::clone(render_ctx);
    let preview_state_discard = Arc::clone(preview_state);
    let solid_last_solved_discard = Arc::clone(solid_last_solved);
    let db_discard = Arc::clone(db);
    let source_discard = Arc::clone(source);
    let ui_weak_discard = ui.as_weak();
    ui.global::<EditorModel>()
        .on_unsaved_dialog_discard(move || {
            let Some(ui) = ui_weak_discard.upgrade() else {
                return;
            };
            ui.global::<EditorModel>().set_unsaved_dialog_open(false);
            let pending = state_discard.borrow_mut().pending_unsaved_action.take();
            run_pending_unsaved_action(
                pending,
                &ui,
                &state_discard,
                &RenderPreviewHandles {
                    render_ctx: &render_ctx_discard,
                    preview_state: &preview_state_discard,
                    solid_last_solved: &solid_last_solved_discard,
                },
                &db_discard,
                &source_discard,
            );
        });

    let state_cancel = Rc::clone(state);
    ui.global::<EditorModel>()
        .on_unsaved_dialog_cancel(move || {
            state_cancel.borrow_mut().pending_unsaved_action = None;
        });
}

#[cfg(test)]
mod tests {
    use super::discards_edits_since;

    #[test]
    fn edits_since_the_decision_are_not_discarded_silently() {
        assert!(
            discards_edits_since(4, 5, true),
            "the generation moved and the design is unsaved"
        );
    }

    #[test]
    fn an_unchanged_generation_is_the_design_the_cutter_already_answered_for() {
        assert!(
            !discards_edits_since(4, 4, true),
            "a Discard that resumed the load leaves the design dirty by choice"
        );
    }

    #[test]
    fn edits_undone_back_to_the_saved_state_need_no_question() {
        assert!(!discards_edits_since(4, 6, false));
    }
}
