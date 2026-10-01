//! "Save Native": writes the design's `.indicatrix.toml` sidecar ALONGSIDE a real
//! `.asc` export -- never instead of one. That pairing is the module's own hard rule
//! (see `indicatrix_cut_core::native`'s module doc comment): a design must never
//! exist only in a format its author can be locked out of, so this always writes
//! both files, never the native file alone. See this group's own `mod.rs` doc
//! comment.

use super::{
    CURRENT_NATIVE_PATH,
    atomic_write::{WriteGate, write_pair_atomically},
    autosave::setup_autosave_timer,
    catalogue::write_back_to_catalogue,
    confirm::{
        StatusDecision, ask_write_confirm, confirm_keys, decide_write_status,
        setup_write_confirm_dialog_callbacks,
    },
    picker::{PickKind, pick_file, suggested_file_name},
    save_finish::{WriteNativeOutcome, finish_save_native_success},
    save_helpers::{
        custom_material_snapshot_for_save, degenerate_marker_header, save_paired_reusing_solve,
        snapshot_custom_materials, stamp_source_entry_footnote,
    },
    solve::{SolveFailure, resolve_solve_at},
};
use crate::{
    EditorModel, MainWindow,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::{editor::state::EditorState, show_toast},
};
use indicatrix::geometry::{meet_solver::SolvedTier, stone_metrics::ExternalProportions};
use indicatrix_cut_core::{
    Design,
    native::{PairedSave, SaveExtras},
    native_path_for_asc,
};
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Instant,
};

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
    /// `EditorState::generation`'s value at the moment `design` was cloned out of
    /// `state` (`quick_save_native`/`save_native_via_dialog`, both well before
    /// `resolve_solved_then`'s background solve and the write itself, which can
    /// together take several seconds on a large design) -- carried through
    /// [`WriteNativeOutcome`] so [`finish_save_native_success`] can mark the design
    /// clean at the generation it ACTUALLY saved, not whatever `generation` reads
    /// once the write finally lands. Using the live generation there instead
    /// let edits made while the save was still resolving/writing read as clean
    /// the moment the (already stale) save completed, so the close guard never
    /// prompted for them and they were lost with no warning.
    snapshot_generation: u64,
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
    /// The single-writer gate in front of [`start_native_write_thread`]: a Save that
    /// reaches the disk write while an earlier one is still writing is parked here
    /// (the newest wins) and started when the earlier one reports in, instead of
    /// racing it on the same files.
    static NATIVE_WRITE_GATE: RefCell<WriteGate<(NativeSaveContext, PairedSave)>> =
        const { RefCell::new(WriteGate::new()) };
}

/// Group 1+2's shared tail for both [`quick_save_native`] and
/// [`save_native_via_dialog`] once `ctx.dest_path`/`ctx.native_path` are known and
/// any overwrite confirmation has already been granted: resolves `design`'s write
/// status off the UI thread ([`resolve_solve_at`]/[`decide_write_status`]), asks
/// [`ask_write_confirm`] only when it names a problem, then hands off to
/// [`write_native_save`].
///
/// A solve that was displaced by a newer request ([`SolveFailure::Superseded`]) says
/// nothing about the geometry: it neither offers the "not a closed solid" prompt nor
/// reaches the file header. The save is abandoned with a toast and nothing is written.
fn finish_native_save(ui: &MainWindow, design: Design, ctx: NativeSaveContext) {
    // Tagged with `ctx.snapshot_generation` -- see `native_io::solve::resolve_solve_at`'s
    // own doc comment: a Save must never write a cached solve for an OLDER generation
    // than the one it just cloned `design` out of.
    let snapshot_generation = ctx.snapshot_generation;
    resolve_solve_at(
        ui,
        Arc::new(design),
        snapshot_generation,
        move |ui, design, solved_result| {
            crate::gui::editor::stall_guard::stall_guard("native_save_status_resolved", || {
                let solved_result = match solved_result {
                    Ok(solved) => Ok(solved),
                    Err(SolveFailure::Failed(message)) => Err(message),
                    Err(SolveFailure::Superseded) => {
                        // This save is not landing -- an `after_save` continuation
                        // stashed right before it must not be left dangling for some
                        // LATER, unrelated save to stumble onto; see `AfterSave`'s
                        // own doc comment.
                        ctx.state.borrow_mut().after_save = None;
                        show_toast(
                            ui,
                            "Save was interrupted by a newer save or export. Nothing was \
                             written; save again.",
                            "error",
                        );
                        return;
                    }
                };
                let solved = solved_result.as_ref().ok().cloned();
                match decide_write_status(&design, solved_result.as_deref().map_err(String::as_str))
                {
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
                                write_native_save(
                                    ui,
                                    &design,
                                    solved.as_deref(),
                                    Some(&message),
                                    &ctx,
                                );
                            },
                        );
                    }
                }
            });
        },
    );
}

/// Parks `state` in [`PENDING_NATIVE_SAVE_STATE`] under a fresh key and returns
/// that key. `Rc<RefCell<EditorState>>` is not `Send` -- it must never cross into
/// [`write_native_save`]'s spawned thread or its `upgrade_in_event_loop` hop back.
/// Stashed in a UI-thread-only rendezvous instead, the same trick `pick_file`'s own
/// `PENDING_PICKS` uses (see that function's own doc comment) -- only the plain
/// `u64` key crosses the thread boundary.
fn stash_native_save_state(state: &Rc<RefCell<EditorState>>) -> u64 {
    let state_key = NEXT_NATIVE_SAVE_KEY.with(|c| {
        let key = c.get();
        c.set(key + 1);
        key
    });
    PENDING_NATIVE_SAVE_STATE.with(|cell| {
        cell.borrow_mut().insert(state_key, Rc::clone(state));
    });
    state_key
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
            // This save is not landing -- an `after_save` continuation stashed
            // right before it must not be left dangling for some LATER,
            // unrelated save to stumble onto; see `AfterSave`'s own doc
            // comment.
            ctx.state.borrow_mut().after_save = None;
            show_toast(ui, &format!("Cannot save: {e}"), "error");
            return;
        }
    };
    spawn_native_save_write(ui, ctx, paired);
}

/// [`write_native_save`]'s background-thread tail, split out purely to keep that
/// function itself under clippy's `too_many_lines` lint: hands `paired` to the
/// single-writer gate ([`NATIVE_WRITE_GATE`]). When no other save is writing it starts
/// now ([`start_native_write_thread`]); otherwise it is parked -- the newest parked
/// save wins -- and starts when the running one reports in, so two quick saves never
/// race on the same files.
fn spawn_native_save_write(ui: &MainWindow, ctx: &NativeSaveContext, paired: PairedSave) {
    let admitted = NATIVE_WRITE_GATE.with(|gate| {
        gate.borrow_mut()
            .admit((ctx.clone(), paired), Instant::now())
    });
    if let Some((ctx, paired)) = admitted {
        start_native_write_thread(ui, &ctx, paired);
    }
}

/// Runs on the UI thread when a save's writer reports in: frees the gate and starts
/// the save parked behind it, if any.
fn release_native_write_gate(ui: &MainWindow) {
    let next = NATIVE_WRITE_GATE.with(|gate| gate.borrow_mut().release(Instant::now()));
    if let Some((ctx, paired)) = next {
        start_native_write_thread(ui, &ctx, paired);
    }
}

/// Spawns the thread that does the actual disk write ([`write_pair_atomically`]) and
/// catalogue write-back ([`write_back_to_catalogue`]), then reports the outcome back on
/// the UI thread via [`finish_save_native_success`] or an error toast, and finally
/// frees the writer gate.
fn start_native_write_thread(ui: &MainWindow, ctx: &NativeSaveContext, paired: PairedSave) {
    let dest_path = ctx.dest_path.clone();
    let native_path = ctx.native_path.clone();
    let asc_filename = ctx.asc_filename.clone();
    let snapshot_generation = ctx.snapshot_generation;
    let db = Arc::clone(&ctx.db);
    let db_for_finish = Arc::clone(&ctx.db);
    let source_for_finish = Arc::clone(&ctx.source);
    let source_entry_id = ctx.state.borrow().source_entry_id;
    let state_key = stash_native_save_state(&ctx.state);
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
                // See `NativeSaveContext::snapshot_generation`'s own doc
                // comment -- carried straight through, unread by anything on
                // this background thread.
                snapshot_generation,
            }
        });
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            report_native_save_outcome(&ui, state_key, outcome, &db_for_finish, &source_for_finish);
            release_native_write_gate(&ui);
        });
    });
}

/// [`start_native_write_thread`]'s UI-thread tail: hands a landed write to
/// [`finish_save_native_success`], or toasts the failure.
fn report_native_save_outcome(
    ui: &MainWindow,
    state_key: u64,
    outcome: Result<WriteNativeOutcome, String>,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let Some(state) = PENDING_NATIVE_SAVE_STATE.with(|cell| cell.borrow_mut().remove(&state_key))
    else {
        return;
    };
    match outcome {
        Ok(outcome) => {
            crate::gui::editor::stall_guard::stall_guard("native_save_write_complete", || {
                finish_save_native_success(ui, &state, outcome, db, source);
            });
        }
        Err(message) => {
            // This save did not land -- an `after_save` continuation stashed right
            // before it must not be left dangling for some LATER, unrelated save to
            // stumble onto; see `AfterSave`'s own doc comment.
            state.borrow_mut().after_save = None;
            show_toast(ui, &message, "error");
        }
    }
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
        snapshot_generation,
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
            // `generation` at THIS exact moment -- the same instant `design` is
            // cloned out of `st`, well before `finish_native_save`'s own
            // background solve/write -- see `NativeSaveContext::
            // snapshot_generation`'s own doc comment.
            st.generation.load(Ordering::Relaxed),
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
            snapshot_generation,
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
        snapshot_generation,
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
            // See `save_native_via_dialog`'s sibling `quick_save_native`'s
            // matching comment -- captured here too, before the
            // Save-As picker even opens.
            st.generation.load(Ordering::Relaxed),
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
                // No save is landing after all -- an `after_save` continuation
                // stashed by a close/replace guard right before this Save-As
                // must not be left dangling for some LATER, unrelated save to
                // stumble onto; see `AfterSave`'s own doc comment.
                state_for_pick.borrow_mut().after_save = None;
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
                        snapshot_generation,
                    },
                );
            });
        },
    );
}

/// `db`/`source`: Save Native's own catalogue write-back needs them (see
/// [`write_back_to_catalogue`]), threaded in from
/// `gui::editor::setup_editor_callbacks`'s call site.
pub(in crate::gui::editor) fn setup_save_native_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    // Stashes `state` for `request_save_then_close` (`gui::window_close`'s
    // close-confirm guard) to reach later -- see that function's own doc
    // comment. There is only ever one `EditorState` for the app's lifetime, so
    // one call here covers every later close.
    super::remember_editor_state(state);
    setup_dirty_tracking(ui, state);
    setup_autosave_timer(ui, state, db, render_ctx);
    // Group 2: the write-confirm dialog's own two callbacks -- bundled in here for
    // the same reason `setup_dirty_tracking`/`setup_autosave_timer` are.
    setup_write_confirm_dialog_callbacks(ui, state);
    let state_save = Rc::clone(state);
    let db_save = Arc::clone(db);
    let source_save = Arc::clone(source);
    let render_ctx_save = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_save_native(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        crate::gui::editor::stall_guard::stall_guard("save_native", || {
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
        crate::gui::editor::stall_guard::stall_guard("save_native_as", || {
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
        // `try_borrow`, never `borrow`: Slint queues `changed tiers` handlers to the
        // next event-loop pass rather than running them inside `set_tiers`, but a
        // caller that pushed the tier list while holding `state.borrow_mut()` and then
        // pumps the event loop still lets one run under that borrow. A plain
        // `borrow()` would panic there with "already mutably borrowed".
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
