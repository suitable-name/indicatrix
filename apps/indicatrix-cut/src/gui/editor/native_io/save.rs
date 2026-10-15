//! "Save"/"Save As": writes the design's self-contained `.isdd` file. The file
//! carries the whole design, so nothing is written beside it; the catalogue row the
//! design belongs to still gets its cutting instructions (`.asc`) and the design file
//! attached ([`write_back_to_catalogue`]). See this group's own `mod.rs` doc comment.

use super::{
    CURRENT_NATIVE_PATH,
    atomic_write::{WriteGate, write_file_atomically},
    autosave::setup_autosave_timer,
    catalogue::write_back_to_catalogue,
    confirm::{
        StatusDecision, ask_write_confirm, confirm_keys, decide_write_status,
        setup_write_confirm_dialog_callbacks,
    },
    design_meta::prepare_design_extras,
    design_paths::{
        ensure_design_extension, path_belongs_to_schedule_name, schedule_name_for_design_path,
    },
    picker::{PickKind, pick_file, suggested_design_file_name},
    save_finish::{WriteNativeOutcome, finish_save_native_success},
    save_helpers::{
        catalogue_asc_reusing_solve, custom_material_snapshot_for_save, degenerate_marker_header,
        design_file_text, snapshot_custom_materials, stamp_source_entry_footnote,
    },
    solve::{SolveFailure, resolve_solve_at},
};
use crate::{
    EditorModel, MainWindow,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::{
        editor::state::{DesignFileExtras, EditorState},
        show_toast,
    },
};
use indicatrix::{
    geometry::{meet_solver::SolvedTier, stone_metrics::ExternalProportions},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    Design,
    native::{CatalogueAsc, DesignExtras, SaveExtras},
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

/// This design's own already-known save location: the remembered `.isdd` file
/// ([`CURRENT_NATIVE_PATH`]), provided it belongs to this design -- its stem is the
/// design's recorded name (`asc_filename`). Without a known location Ctrl+S would
/// always reopen the Save-As dialog, even seconds after the cutter had just chosen
/// one; without the name check a design loaded over another one (a catalogue "Load
/// Selected") would overwrite the previous design's file.
///
/// `None` falls through to [`save_native_via_dialog`]'s ordinary Save-As behaviour:
/// a brand-new design, a catalogue design, a bare `.asc`, a design opened from a legacy
/// `.indicatrix` file (its first Save proposes the `.isdd` name beside it and leaves
/// the old file alone) or a recovered autosave.
fn known_save_target(st: &EditorState) -> Option<PathBuf> {
    let path = CURRENT_NATIVE_PATH.with(|cell| cell.borrow().clone())?;
    let name = st.asc_filename.as_deref()?;
    path_belongs_to_schedule_name(&path, name).then_some(path)
}

/// Every write/export/autosave path in this module that ends up calling
/// [`catalogue_asc_reusing_solve`] needs this same bundle of `state` snapshot data
/// plus the destination paths and shared handles -- grouped into one `Clone`
/// struct (rather than nine-plus parameters threaded through several async
/// continuations) so [`finish_native_save`]/[`write_native_save`] can move a single
/// owned copy into whichever branch (confirmed or not) ends up running, and again
/// into the background thread that does the actual write.
#[derive(Clone)]
struct NativeSaveContext {
    state: Rc<RefCell<EditorState>>,
    /// The `.isdd` file being written.
    native_path: PathBuf,
    db: Arc<Mutex<Database>>,
    source: Arc<Mutex<LibrarySource>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    /// The design's recorded schedule name (`<stem>.asc`): the catalogue row's file
    /// name and the editor's default export name; no such file is written here.
    asc_filename: String,
    original_asc_text: Option<String>,
    printed_proportions: Option<ExternalProportions>,
    used_placeholder: bool,
    history_entries: Vec<String>,
    /// The `[meta]` table and attachments the design came with -- the base a Save
    /// overlays the library row onto, see `design_meta`.
    file_extras: DesignFileExtras,
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
    /// Saves whose worker-side preparation ([`prepare_native_save`]) is still running, by
    /// sequence number: the `Rc`-holding context stays here on the UI thread and only the
    /// number crosses the thread boundary, the same trick as [`PENDING_NATIVE_SAVE_STATE`].
    static PENDING_NATIVE_PREPARE: RefCell<std::collections::HashMap<u64, NativeSaveContext>> =
        RefCell::new(std::collections::HashMap::new());
    /// The sequence number of the newest Save started; numbers begin at 1.
    static PREPARE_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The sequence number of the newest prepared Save that reached the writer gate, so an
    /// older preparation finishing late is dropped instead of overwriting a newer file.
    static LAST_LANDED_PREPARE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The single-writer gate in front of [`start_native_write_thread`]: a Save that
    /// reaches the disk write while an earlier one is still writing is parked here
    /// (the newest wins) and started when the earlier one reports in, instead of
    /// racing it on the same files.
    static NATIVE_WRITE_GATE: RefCell<WriteGate<(NativeSaveContext, CatalogueAsc, String)>> =
        const { RefCell::new(WriteGate::new()) };
}

/// Forgets the saves a previous main window left in flight: the editor states parked for a
/// running write, the prepared-save contexts (each holds the design library handle) and the
/// write gate's parked request. A write thread still running only finds nothing to report to.
/// See `gui::window_release`.
pub(super) fn release_window_state() {
    let states = PENDING_NATIVE_SAVE_STATE.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    let prepared = PENDING_NATIVE_PREPARE.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    let gate =
        NATIVE_WRITE_GATE.with(|cell| std::mem::replace(&mut *cell.borrow_mut(), WriteGate::new()));
    // Dropped with every cell released.
    drop(states);
    drop(prepared);
    drop(gate);
}

/// Names this module's holders in `out` while they are still set. See
/// `gui::window_release::live_holders`.
#[cfg(feature = "ui-harness")]
pub(super) fn note_holders(out: &mut Vec<&'static str>) {
    if PENDING_NATIVE_PREPARE.with(|cell| !cell.borrow().is_empty()) {
        out.push("editor::native_io::save PENDING_NATIVE_PREPARE (vault)");
    }
    if PENDING_NATIVE_SAVE_STATE.with(|cell| !cell.borrow().is_empty()) {
        out.push("editor::native_io::save PENDING_NATIVE_SAVE_STATE");
    }
}

/// Group 1+2's shared tail for both [`quick_save_native`] and
/// [`save_native_via_dialog`] once `ctx.native_path` is known and
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
                            "This Design Is Not a Closed Solid",
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

/// Everything [`prepare_native_save`] needs, as plain `Send` data: the UI-thread part of a
/// Save is only this snapshot, because the library-row read (every attachment BLOB), the
/// custom-material row read and the text serialisation together are tens of milliseconds to
/// seconds on a design with multi-MB attachments, under the DB mutex other workers wait on.
struct SavePrepInput {
    design: Design,
    solved: Option<Vec<SolvedTier>>,
    placeholder_note: Option<&'static str>,
    db: Arc<Mutex<Database>>,
    source_entry_id: Option<i64>,
    file_extras: DesignFileExtras,
    original_asc_text: Option<String>,
    printed_proportions: Option<ExternalProportions>,
    history_entries: Vec<String>,
    custom_materials: Arc<Vec<GemMaterial>>,
}

/// What [`prepare_native_save`] hands back to the UI thread.
struct PreparedNativeSave {
    /// The catalogue row's cutting instructions and the draft verdict.
    asc: CatalogueAsc,
    design_text: String,
    /// Remembered on the editor state so the next Save starts from what this one wrote --
    /// above all the generated `id`, which must not change from one Save to the next.
    file_extras: DesignFileExtras,
}

/// The worker half of a Save: reads the custom-material row and the library row (locking the
/// DB mutex here, never on the UI thread), builds the `.asc` and the design file's text. No
/// disk write happens here. The same calls on the same inputs in the same order as the
/// UI-thread version this replaced.
///
/// # Errors
///
/// A ready-to-toast message: the row could not be read, the attachments exceed the file's
/// limit, or the serialiser refused a value.
fn prepare_native_save(input: SavePrepInput) -> Result<PreparedNativeSave, String> {
    let SavePrepInput {
        design,
        solved,
        placeholder_note,
        db,
        source_entry_id,
        file_extras,
        original_asc_text,
        printed_proportions,
        history_entries,
        custom_materials,
    } = input;
    // See `custom_material_snapshot_for_save`'s own doc comment.
    let custom_material = custom_material_snapshot_for_save(&design, &db);
    let extras = SaveExtras {
        custom_material: custom_material.as_ref(),
        history_entries: &history_entries,
        custom_catalogue: &custom_materials,
    };
    // The `[meta]` table and attachments: the design's loaded ones overlaid with its
    // library row. Oversize attachments end the save here with a message.
    let prepared_extras = prepare_design_extras(&db, source_entry_id, &file_extras)?;
    let design_extras = DesignExtras {
        metadata: Some(&prepared_extras.metadata),
        attachments: &prepared_extras.attachments,
        ..DesignExtras::from(&extras)
    };
    let asc = catalogue_asc_reusing_solve(
        &design,
        solved.as_deref(),
        original_asc_text.as_deref(),
        placeholder_note,
        &extras,
    )
    .map_err(|e| e.to_string())?;
    let design_text = design_file_text(
        &design,
        printed_proportions.as_ref(),
        &design_extras,
        asc.draft_reason.is_some(),
    )?;
    Ok(PreparedNativeSave {
        asc,
        design_text,
        file_extras: DesignFileExtras::new(prepared_extras.metadata, prepared_extras.attachments),
    })
}

/// [`finish_native_save`]'s write half: snapshots the state the save needs on the UI thread,
/// builds the catalogue's `.asc` and the design file's text on a worker
/// ([`prepare_native_save`]), and then -- back on the UI thread -- hands the actual disk
/// write ([`write_file_atomically`]) and catalogue write-back ([`write_back_to_catalogue`]) to
/// a second background thread (Group 4: "files first then catalogue, results back via the
/// event loop"). `header_message` is `Some` only when the cutter just confirmed a "not a
/// closed solid" write; stamped into `design.meta.headers` before anything is serialized, so
/// the file always describes the bytes actually written, never mutated after the fact. The
/// file's `draft` flag is set when the design did not solve.
///
/// Preparation results can land out of order when two Saves are issued back to back; a
/// result older than one that already landed is dropped (see [`land_prepared_native_save`]),
/// the same "newest wins" rule the writer gate applies to parked writes.
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
    let input = SavePrepInput {
        design,
        solved: solved.map(<[SolvedTier]>::to_vec),
        // A design reconstructed from a catalogue's bare angle table has every mast
        // fabricated as `0.0`. `catalogue_asc` stamps
        // `indicatrix_formats::asc::mark_reconstructed` when told so, which is what stops
        // the catalogue's `.asc` from passing for a real cut instruction.
        placeholder_note: ctx
            .used_placeholder
            .then_some("angle-table reconstruction, no attached .asc"),
        db: Arc::clone(&ctx.db),
        source_entry_id: ctx.state.borrow().source_entry_id,
        file_extras: ctx.file_extras.clone(),
        original_asc_text: ctx.original_asc_text.clone(),
        printed_proportions: ctx.printed_proportions,
        history_entries: ctx.history_entries.clone(),
        // Custom-catalogue-aware -- see `snapshot_custom_materials`'s own doc comment.
        custom_materials: snapshot_custom_materials(&ctx.render_ctx),
    };
    let seq = PREPARE_SEQ.with(|c| {
        let seq = c.get() + 1;
        c.set(seq);
        seq
    });
    PENDING_NATIVE_PREPARE.with(|cell| {
        cell.borrow_mut().insert(seq, ctx.clone());
    });
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || {
        let result = prepare_native_save(input);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            land_prepared_native_save(&ui, seq, result);
        });
    });
}

/// [`write_native_save`]'s UI-thread tail: starts the disk write for a prepared save, or
/// toasts why none can be written. A result older than one that already landed is dropped.
fn land_prepared_native_save(
    ui: &MainWindow,
    seq: u64,
    result: Result<PreparedNativeSave, String>,
) {
    let Some(ctx) = PENDING_NATIVE_PREPARE.with(|cell| cell.borrow_mut().remove(&seq)) else {
        return;
    };
    if LAST_LANDED_PREPARE.with(std::cell::Cell::get) > seq {
        return;
    }
    LAST_LANDED_PREPARE.with(|c| c.set(seq));
    match result {
        Ok(prepared) => {
            ctx.state.borrow_mut().file_extras = prepared.file_extras;
            spawn_native_save_write(ui, &ctx, prepared.asc, prepared.design_text);
        }
        Err(e) => {
            // This save is not landing -- an `after_save` continuation stashed
            // right before it must not be left dangling for some LATER,
            // unrelated save to stumble onto; see `AfterSave`'s own doc
            // comment.
            ctx.state.borrow_mut().after_save = None;
            show_toast(ui, &format!("Cannot save: {e}"), "error");
        }
    }
}

/// [`write_native_save`]'s background-thread tail, split out purely to keep that
/// function itself under clippy's `too_many_lines` lint: hands the save to the
/// single-writer gate ([`NATIVE_WRITE_GATE`]). When no other save is writing it starts
/// now ([`start_native_write_thread`]); otherwise it is parked -- the newest parked
/// save wins -- and starts when the running one reports in, so two quick saves never
/// race on the same file.
fn spawn_native_save_write(
    ui: &MainWindow,
    ctx: &NativeSaveContext,
    asc: CatalogueAsc,
    design_text: String,
) {
    let admitted = NATIVE_WRITE_GATE.with(|gate| {
        gate.borrow_mut()
            .admit((ctx.clone(), asc, design_text), Instant::now())
    });
    if let Some((ctx, asc, design_text)) = admitted {
        start_native_write_thread(ui, &ctx, asc, design_text);
    }
}

/// Runs on the UI thread when a save's writer reports in: frees the gate and starts
/// the save parked behind it, if any.
fn release_native_write_gate(ui: &MainWindow) {
    let next = NATIVE_WRITE_GATE.with(|gate| gate.borrow_mut().release(Instant::now()));
    if let Some((ctx, asc, design_text)) = next {
        start_native_write_thread(ui, &ctx, asc, design_text);
    }
}

/// Spawns the thread that does the actual disk write ([`write_file_atomically`]) and
/// catalogue write-back ([`write_back_to_catalogue`]), then reports the outcome back on
/// the UI thread via [`finish_save_native_success`] or an error toast, and finally
/// frees the writer gate.
fn start_native_write_thread(
    ui: &MainWindow,
    ctx: &NativeSaveContext,
    asc: CatalogueAsc,
    design_text: String,
) {
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
        if let Some(parent) = native_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let write_result = write_file_atomically(&native_path, &design_text);
        let outcome = write_result.map(|()| {
            let design_filename = native_path
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
                &asc.asc_text,
                &design_filename,
                &design_text,
            );
            WriteNativeOutcome {
                native_path: native_path.clone(),
                asc_filename: asc_filename.clone(),
                asc,
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

/// Quick save: writes straight to `native_path` (already known -- see
/// [`known_save_target`]) with no file dialog. Otherwise identical to
/// [`save_native_via_dialog`]'s own tail: the same degenerate-status confirmation, the
/// same atomic write, the same success/failure reporting -- both funnel through
/// [`finish_native_save`].
fn quick_save_native(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    native_path: &Path,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let (mut design, saved) = {
        let st = state.borrow();
        (st.design.clone(), SavedState::capture(&st))
    };
    // See `stamp_source_entry_footnote`'s own doc comment.
    stamp_source_entry_footnote(&mut design.meta.footnotes, saved.source_entry_id);
    finish_native_save(
        ui,
        design,
        saved.into_context(
            state,
            native_path.to_path_buf(),
            db,
            source,
            render_ctx,
            None,
        ),
    );
}

/// The `state` data a save snapshots before its (asynchronous) solve and write start.
struct SavedState {
    asc_filename: Option<String>,
    suggested_name: String,
    original_asc_text: Option<String>,
    printed_proportions: Option<ExternalProportions>,
    source_entry_id: Option<i64>,
    used_placeholder: bool,
    history_entries: Vec<String>,
    file_extras: DesignFileExtras,
    /// `generation` at the moment the design was cloned -- see
    /// [`NativeSaveContext::snapshot_generation`]'s own doc comment.
    snapshot_generation: u64,
}

impl SavedState {
    /// Snapshots everything a save reads from `st`.
    fn capture(st: &EditorState) -> Self {
        Self {
            asc_filename: st.asc_filename.clone(),
            suggested_name: suggested_design_file_name(st),
            original_asc_text: st.original_asc_text.clone(),
            printed_proportions: st.printed_proportions,
            source_entry_id: st.source_entry_id,
            used_placeholder: st.used_placeholder,
            // Carried on every native save -- see `SaveExtras::history_entries`'s
            // own doc comment.
            history_entries: st.history.description_log().to_vec(),
            file_extras: st.file_extras.clone(),
            snapshot_generation: st.generation.load(Ordering::Relaxed),
        }
    }

    /// The write context for saving to `native_path`. `schedule_name` overrides the
    /// recorded schedule name (a Save As names the design after the new file); `None`
    /// keeps the design's own, which a quick save always has (`known_save_target`
    /// requires it).
    fn into_context(
        self,
        state: &Rc<RefCell<EditorState>>,
        native_path: PathBuf,
        db: &Arc<Mutex<Database>>,
        source: &Arc<Mutex<LibrarySource>>,
        render_ctx: &Arc<Mutex<RenderContext>>,
        schedule_name: Option<String>,
    ) -> NativeSaveContext {
        NativeSaveContext {
            state: Rc::clone(state),
            native_path,
            db: Arc::clone(db),
            source: Arc::clone(source),
            render_ctx: Arc::clone(render_ctx),
            asc_filename: schedule_name.or(self.asc_filename).unwrap_or_default(),
            original_asc_text: self.original_asc_text,
            printed_proportions: self.printed_proportions,
            used_placeholder: self.used_placeholder,
            history_entries: self.history_entries,
            file_extras: self.file_extras,
            snapshot_generation: self.snapshot_generation,
        }
    }
}

/// Save-As: always shows the native save dialog. Reached two ways: as
/// [`setup_save_native_callback`]'s own fallback, for a design
/// [`known_save_target`] has no answer for yet, and as `EditorModel.save_native_as`'s
/// entire body (a dedicated callback backed by its own
/// `ui/models/editor.slint`/`ui/app.slint` wiring) for a cutter who explicitly
/// wants to save the current design to a DIFFERENT file.
///
/// The dialog is the OS's own save dialog, which already asks before replacing an
/// existing file; the replaced copy is also kept as a `.bak`. The design is recorded
/// under the chosen file's name from then on.
fn save_native_via_dialog(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let (mut design, saved) = {
        let st = state.borrow();
        (st.design.clone(), SavedState::capture(&st))
    };
    // See `stamp_source_entry_footnote`'s own doc comment.
    stamp_source_entry_footnote(&mut design.meta.footnotes, saved.source_entry_id);

    // Group 3: the save-as picker runs on a background thread -- see
    // `pick_file`'s own doc comment. `state` is not borrowed across this call.
    let state_for_pick = Rc::clone(state);
    let db_for_pick = Arc::clone(db);
    let source_for_pick = Arc::clone(source);
    let render_ctx_for_pick = Arc::clone(render_ctx);
    let default_name = saved.suggested_name.clone();
    pick_file(
        ui,
        PickKind::SaveDesign { default_name },
        move |ui, picked| {
            // See the matching comment on `setup_export_asc_callback`'s own
            // save-picker cancel -- a dismissed dialog needs no toast.
            let Some(picked) = picked else {
                // No save is landing after all -- an `after_save` continuation
                // stashed by a close/replace guard right before this Save-As
                // must not be left dangling for some LATER, unrelated save to
                // stumble onto; see `AfterSave`'s own doc comment.
                state_for_pick.borrow_mut().after_save = None;
                return;
            };
            let native_path = ensure_design_extension(picked);
            let schedule_name = schedule_name_for_design_path(&native_path);
            finish_native_save(
                ui,
                design,
                saved.into_context(
                    &state_for_pick,
                    native_path,
                    &db_for_pick,
                    &source_for_pick,
                    &render_ctx_for_pick,
                    Some(schedule_name),
                ),
            );
        },
    );
}

/// `db`/`source`: Save's own catalogue write-back needs them (see
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
    setup_autosave_timer(ui, state, db);
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
            // one exists, so Ctrl+S/"Save" stop being a Save-As round trip
            // on every press -- only a design with no known location yet (see
            // `known_save_target`'s own doc comment) still shows the dialog.
            let known = {
                let st = state_save.borrow();
                known_save_target(&st)
            };
            match known {
                Some(native_path) => {
                    quick_save_native(
                        &ui,
                        &state_save,
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

    // "Save As...": always shows the dialog, even when a known location
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
