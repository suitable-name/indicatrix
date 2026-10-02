//! [`EditorState`] itself: its fields, construction (`fresh`/`fresh_from_template`),
//! wholesale replacement, and the pending-action/pending-remap payload types its
//! fields hold.

use crate::{
    gui::editor::{deep_solve, optimize_solve, retarget},
    settings::SettingsPersister,
};
use indicatrix_cut_core::{FreshDesignSpec, OptimizeOutcome, RemapRounding};
use indicatrix_editor::{EditorSession, material::MaterialComboCache, scratch::PushedScratch};
use std::{
    cell::RefCell,
    ops::{Deref, DerefMut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
};
use tracing::warn;

use super::file_extras::DesignFileExtras;

/// The coalescing window every `EditorState`-owned `History` is built with -- see
/// [`indicatrix_editor::session::ANGLE_NUDGE_COALESCE_WINDOW`], re-exported here at
/// its old path.
pub(in crate::gui::editor) use indicatrix_editor::session::ANGLE_NUDGE_COALESCE_WINDOW;

/// The `AppSettings::suppressed_confirmations` key the anchor explainer card's
/// "Don't show again" persists.
const ANCHOR_EXPLAINER_SUPPRESS_KEY: &str = "anchor_explainer";

thread_local! {
    /// Whether this session has already DECIDED once (shown the card, or
    /// found it suppressed) whether to open the anchor explainer -- on top of
    /// the permanent "Don't show again" suppression persisted in
    /// `AppSettings` (see [`ANCHOR_EXPLAINER_SUPPRESS_KEY`]), so a design
    /// that stays `MissingAnchor` across several further edits neither
    /// reopens the card NOR re-reads the settings file on every refresh --
    /// only the FIRST time a design lacks an anchor is ever checked at all.
    static ANCHOR_EXPLAINER_DECIDED_THIS_SESSION: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Whether the anchor explainer's "Don't show again" has been recorded, read from the
/// application's [`SettingsPersister`] (see [`SettingsPersister::install_for_this_thread`])
/// so a choice made earlier in this session counts. With no persister installed
/// nothing is suppressed.
///
/// The same pattern as `native_io::confirm`'s write-confirm suppressions, which live
/// in a private module and so cannot be called from here.
fn anchor_explainer_is_suppressed() -> bool {
    SettingsPersister::installed_for_this_thread().is_some_and(|persister| {
        persister
            .snapshot()
            .settings
            .is_confirm_suppressed(ANCHOR_EXPLAINER_SUPPRESS_KEY)
    })
}

/// Records the anchor explainer's "Don't show again" -- the write half of
/// [`anchor_explainer_is_suppressed`]. Goes through the [`SettingsPersister`], never
/// straight into the settings file: the persister's in-memory snapshot is what every
/// close path flushes over the file, so a choice written around it would be erased on
/// exit.
pub(in crate::gui::editor) fn anchor_explainer_suppress_permanently() {
    let Some(persister) = SettingsPersister::installed_for_this_thread() else {
        warn!("No settings persister is installed; not suppressing the anchor explainer");
        return;
    };
    persister.update(|file| {
        file.settings
            .suppress_confirm(ANCHOR_EXPLAINER_SUPPRESS_KEY);
    });
}

/// Whether the anchor explainer card should open NOW, given
/// `design_has_missing_anchor` (this refresh's own solve result: `true` when
/// [`Design::solve`] just returned `Err(MissingAnchor)`) -- session-scoped
/// once-only (see [`ANCHOR_EXPLAINER_DECIDED_THIS_SESSION`]) and permanently
/// suppressible (see [`anchor_explainer_is_suppressed`]). Marks the session
/// flag the moment it makes ANY decision (open or not), not only when it
/// opens, so a design that stays `MissingAnchor` (or one whose explainer is
/// already permanently suppressed) never touches the settings file again
/// after the first check this session -- every refresh after that is a
/// cheap `Cell` read, not a file read.
#[must_use]
pub(in crate::gui::editor) fn should_open_anchor_explainer(
    design_has_missing_anchor: bool,
) -> bool {
    if !design_has_missing_anchor {
        return false;
    }
    if ANCHOR_EXPLAINER_DECIDED_THIS_SESSION.with(std::cell::Cell::get) {
        return false;
    }
    ANCHOR_EXPLAINER_DECIDED_THIS_SESSION.with(|decided| decided.set(true));
    !anchor_explainer_is_suppressed()
}

/// A gear-change the design settings panel has previewed but not yet confirmed --
/// `setup_gear_apply_callback` fills this in and
/// `setup_gear_remap_confirm_callback`/`setup_gear_remap_cancel_callback` are the only
/// two consumers (apply, or discard). `symmetry_order`/`mirror` are the design's own
/// current values at preview time, carried here so confirming needs no second read of
/// `design.meta`.
pub(in crate::gui::editor) struct PendingGearRemap {
    pub(in crate::gui::editor) from_gear: i32,
    pub(in crate::gui::editor) to_gear: i32,
    pub(in crate::gui::editor) symmetry_order: u32,
    pub(in crate::gui::editor) mirror: bool,
    pub(in crate::gui::editor) rounding: RemapRounding,
}

/// A destructive action (New/Load Selected/Open) that was about to replace
/// [`EditorState`] wholesale while the design still had unsaved edits -- stashed by
/// that action's own callback the moment it sees [`EditorState::is_dirty`], so the
/// Save/Discard/Cancel dialog's own resolution (`gui::editor::callbacks::tier_actions::
/// setup_unsaved_guard_dispatch`) knows what to actually run once the user picks
/// Save or Discard (Cancel just clears this back to `None` and does nothing).
///
/// `New` carries the already-parsed [`FreshDesignSpec`] (built from the New Design
/// dialog's fields at the moment "Create" was clicked) rather than re-reading those
/// fields later -- the dialog's fields are `EditorModel` properties that COULD in
/// principle change while the confirmation dialog is up, and re-parsing them then
/// would silently create a different design than the one the user actually asked
/// for. `LoadSelected`/`OpenNative` carry nothing: both re-read their own inputs
/// (the library selection, a freshly-shown file picker) fresh at resume time,
/// which is exactly what running them again from scratch means.
pub(in crate::gui) enum PendingUnsavedAction {
    /// Resume by building this spec into a fresh design -- see
    /// `gui::editor::callbacks::tier_actions::do_new_design_create`.
    ///
    /// `template_index` is the dialog's "Start from" choice,
    /// carried through the unsaved-changes guard rather than re-read afterwards:
    /// the dialog is already closed by the time the guard resolves, so its own
    /// state is gone.
    New {
        spec: FreshDesignSpec,
        template_index: i32,
    },
    /// Resume by re-running "Load Selected" against the library's current selection --
    /// see `gui::editor::callbacks::tier_actions::do_load_selected`.
    LoadSelected,
    /// Resume by re-running "Open" (which shows the file picker again) --
    /// see `gui::editor::native_io::do_open_native`.
    OpenNative,
}

/// What to do once the Save this guard just triggered actually lands -- set by
/// [`EditorState::after_save`]'s two writers (`gui::window_close`'s close-confirm
/// "Save" and [`PendingUnsavedAction`]'s own "Save" resolution,
/// `callbacks::tier_actions::lifecycle::setup_unsaved_guard_dispatch`) right before
/// each calls `EditorModel.save_native`. Both guards used to check
/// `EditorState::is_dirty`/`EditorModel.is_dirty` SYNCHRONOUSLY right after that
/// call returned, as if the save had already finished -- but Save is
/// asynchronous end to end (the design is resolved and the file written on a
/// spawned thread, `native_io::save`'s own `write_native_save`), so that check
/// always read the state from BEFORE the save even started, never its outcome.
/// This field lets the save's own real completion
/// (`native_io::save_finish::finish_save_native_success`, the one place that
/// actually knows the write landed) run the guarded action instead, via
/// `native_io::notify_save_completed`'s listener mechanism -- see that function's
/// own doc comment for why a listener registry, not a parameter threaded through
/// the whole write path.
///
/// Cleared (never left dangling) on every path that does NOT end in a completed
/// save: the Save-As picker's own cancel (`native_io::save::save_native_via_dialog`),
/// the write-confirm dialog's Cancel (`native_io::confirm::
/// setup_write_confirm_dialog_callbacks`), and the background write's own `Err`
/// toast path (`native_io::save::write_native_save`) -- a stale `Some` left behind
/// by any of those would otherwise make some LATER, unrelated successful save
/// spuriously close the window or resume a stale pending action.
///
/// Declared `pub(in crate::gui)`, not the narrower `pub(in crate::gui::editor)`
/// every other type here uses: `gui::window_close` is a SIBLING of `gui::editor`,
/// not a descendant, and needs to name this type directly (see
/// `native_io`'s own re-export of it).
pub(in crate::gui) enum AfterSave {
    /// `gui::window_close`'s close-confirm guard: hide the window once the save
    /// actually lands, instead of the old synchronous `is_dirty` check that raced
    /// the save's own background write thread.
    CloseWindow,
    /// The New/Load Selected/Open unsaved-changes guard: resume this
    /// action once the save actually lands.
    Resume(PendingUnsavedAction),
}

/// The editor's live state: the shared [`EditorSession`] (design, undo/redo history,
/// multi-selection, generation/saved-generation -- every design/history mutation
/// goes through it, so the desktop and the web app edit identically) plus the
/// desktop-only bookkeeping around it (Deep Solve/Optimize handles, the paired
/// `.asc`, the catalogue row, the unsaved-changes guard). Derefs to the session, so
/// `state.design`/`state.history`/`state.multi_selected`/`state.generation` read
/// straight through. See this group's `mod.rs` doc comment for why
/// [`Self::apply`]/[`Self::undo`]/[`Self::redo`] (all routed through the session)
/// are the only functions allowed to touch design and history at once.
///
/// Shared via `Rc<RefCell<..>>`, not this crate's usual `Arc<Mutex<..>>`: every
/// callback that touches this state is a Slint callback, which only ever runs on the
/// UI/event-loop thread -- unlike `RenderContext`, which the render thread also reads
/// every frame and genuinely needs a lock for. A real lock here would buy no
/// correctness while inviting clippy's `significant_drop_tightening` lint on every
/// callback holding the guard across a `refresh_all` call.
pub(in crate::gui) struct EditorState {
    /// The design, its history, the multi-selection and the generation counters --
    /// see [`EditorSession`]'s own field docs (`generation` is an `Arc<AtomicU64>`
    /// so a Deep Solve/Optimize completion handler on a worker thread can notice the
    /// design changed without capturing this non-`Send` state).
    pub(in crate::gui::editor) session: EditorSession,
    /// This design's printed proportions (`Vol/W^3`, `L/W`, `C/W`, `P/W`, `H/W`), when
    /// loaded from a catalogue entry that has them -- Deep Solve's external
    /// verification targets. `None` for a brand-new design or one loaded with those
    /// columns unpopulated; either way Deep Solve must be disabled rather than run
    /// against an unmeasurable target.
    pub(in crate::gui::editor) printed_proportions:
        Option<indicatrix::geometry::stone_metrics::ExternalProportions>,
    /// Bumped ONLY by [`Self::replace_wholesale`] -- i.e. exactly when New / Load
    /// Selected / Open swap in a different design, never by an ordinary
    /// edit.
    ///
    /// `generation` cannot answer this on its own: it counts edits AND
    /// replacements alike, so a background Deep Solve/Optimize comparing against it
    /// learns only "something changed", not whether its result still describes the
    /// design on screen. The two cases want opposite handling. After an edit, a
    /// finished run's verdict is still about this design a few edits ago -- worth
    /// showing with a caveat, since the run took minutes and its aggregate figures
    /// still mean something. After a replacement it is about a different stone
    /// entirely, and the panel is legitimately meant to be empty (see
    /// `callbacks::solve_actions::clear_analysis_results`), so the completion
    /// handler must stay silent rather than repaint a verdict over the fresh
    /// design's blank panel.
    ///
    /// Same `Arc<AtomicU64>` sharing discipline as `generation`, and the same trap:
    /// [`Self::replace_wholesale`] carries the EXISTING `Arc` across the
    /// replacement instead of adopting the replacement's own fresh one, so every
    /// clone a background closure captured before the replacement observes the
    /// bump. Handing back a new `Arc` here would silently defeat the check for
    /// precisely the runs it exists to catch.
    pub(in crate::gui::editor) design_epoch: Arc<AtomicU64>,
    /// See [`PendingUnsavedAction`]. `None` whenever the Save/Discard/Cancel dialog
    /// is closed (the common state).
    pub(in crate::gui::editor) pending_unsaved_action: Option<PendingUnsavedAction>,
    /// See [`AfterSave`]. `None` whenever no Save is currently running on this
    /// design's behalf of a close/replace guard (the common state) -- an ordinary
    /// Save click (with no guard behind it) never touches this field at all.
    pub(in crate::gui) after_save: Option<AfterSave>,
    /// The in-flight Deep Solve's handle, so `setup_deep_solve_cancel_callback` can
    /// reach it -- `None` when none has ever run. Whether one is CURRENTLY running is
    /// tracked by the `editor_deep_solve_running` Slint property, not by this being
    /// `Some`: the completion callback can't clear this field itself, so a finished
    /// run's handle just sits here until overwritten by the next one.
    pub(in crate::gui::editor) deep_solve: Option<deep_solve::DeepSolveHandle>,
    /// The in-flight Optimize run's handle -- same role as `deep_solve` above.
    pub(in crate::gui::editor) optimize: Option<optimize_solve::OptimizeSolveHandle>,
    /// The most recent COMPLETED or CANCELLED Optimize run's result, held here (never
    /// auto-applied) until `setup_optimize_apply_callback` commits it via
    /// [`Self::apply_optimize_outcome`], or a fresh run supersedes it. Paired with the
    /// design `generation` the search ran against, so an Apply click after the design
    /// has since changed is refused rather than reinterpreting stale tier indices.
    ///
    /// `Arc<Mutex<..>>` for the same non-`Send` reason `generation` is atomic --
    /// Optimize's completion handler runs on a worker thread -- but a `Mutex` here
    /// since the payload (a whole [`OptimizeOutcome`]) isn't atomically representable.
    pub(in crate::gui::editor) pending_optimize: Arc<Mutex<Option<(OptimizeOutcome, u64)>>>,
    /// The design `generation`
    /// (see [`Self::generation`]) Deep Solve's currently DISPLAYED verdict
    /// (`EditorModel.deep_solve_status`) was computed for, or `None` before any
    /// Deep Solve has ever completed for this design. Unlike `pending_optimize`'s
    /// matching `u64` (which only needs a ONE-TIME staleness check at Apply time),
    /// this is compared against the LIVE `generation` on every subsequent edit
    /// (`view::push_stale_content`, via [`result_is_stale`]) so the status strip's
    /// "Stale: design changed" badge appears the moment a further edit lands,
    /// not only at the instant the run itself completed -- see
    /// `callbacks::solve_actions::apply_deep_solve_outcome`, the one writer.
    pub(in crate::gui::editor) deep_solve_result_generation: Option<u64>,
    /// The paired `.asc`'s bare file name and exact original text, when this design's
    /// schedule came from a real `.asc` file on disk (a catalogue attachment, or a
    /// previous save/open). `None` for a brand-new design or one reconstructed
    /// from the angle-table placeholder. Fed to [`indicatrix_cut_core::save_paired`]'s
    /// `original_asc_text` parameter, which lets a Save leave the `.asc` half
    /// byte-for-byte untouched instead of regenerating it.
    pub(in crate::gui::editor) asc_filename: Option<String>,
    pub(in crate::gui::editor) original_asc_text: Option<String>,
    /// The catalogue row this design belongs to, once it has one -- see the
    /// "full round trip" design decision below.
    ///
    /// Set when Load Selected opens a LOCAL row, and by the first Save of a
    /// design that had none (which inserts a row and records its id). Every later
    /// save updates that same row rather than inserting a second, which is what
    /// stops export-then-reimport from filling the catalogue with near-duplicate
    /// same-titled entries. `None` for a design that has never been in the
    /// catalogue, and for anything opened from a remote library -- a remote entry id
    /// and a local row id occupy independent id spaces, so storing one here would be
    /// a silent mis-write waiting to happen.
    ///
    /// Deliberately NOT reset by a plain edit: the design keeps belonging to its row
    /// across edits, and only a wholesale replacement (`replace_wholesale`, i.e. New
    /// or a different Load) puts a different design on the bench.
    pub(in crate::gui::editor) source_entry_id: Option<i64>,
    /// Whether `design`'s masts came from `loading::LoadedDesign`'s angle-table
    /// reconstruction fallback -- no attached `.asc` was found, so every mast is a
    /// fabricated `0.0`. Lets Save and Export stamp
    /// `indicatrix_formats::asc::mark_reconstructed`, so a file that looks like a
    /// real cut instruction but is not says so in its own header. `false` for every
    /// other construction path.
    pub(in crate::gui::editor) used_placeholder: bool,
    /// The `[meta]` table and attachments of the `.indicatrix` file this design came
    /// from, so a Save writes them back unchanged -- see [`DesignFileExtras`].
    pub(in crate::gui::editor) file_extras: DesignFileExtras,
    /// See [`PendingGearRemap`]. `None` whenever the gear remap confirmation panel is
    /// closed (the common state).
    pub(in crate::gui::editor) pending_gear_remap: Option<PendingGearRemap>,
    /// The last-built "Retarget for material" proposal, paired with the design
    /// `generation` it was built against -- the same stale-result shape
    /// `pending_optimize` uses, simplified to a plain `Option` since nothing here
    /// completes on a background thread. Cleared by `setup_retarget_apply_callback`
    /// (after applying) and `setup_retarget_close_callback` (on cancel).
    pub(in crate::gui::editor) pending_retarget: Option<(retarget::RetargetProposal, u64)>,
    /// Snapshot of every design-derived value the design-settings/preform/yield
    /// scratch fields mirror, as of the last time `view::refresh_design_settings`/
    /// the preform+yield push in `view::refresh_editor_panel`/`view::
    /// push_stale_content` actually wrote them into `EditorModel` -- see
    /// [`ScratchDelta`]'s own doc comment for why this exists: it stops an
    /// unrelated edit's refresh from silently overwriting whatever the user is
    /// mid-typing/mid-selecting in these fields, since every refresh otherwise
    /// re-seeds all of them from `design` unconditionally.
    ///
    /// A `RefCell`, not a plain field mutated through `&mut self`, purely so
    /// `view`'s refresh functions -- which only ever receive `&EditorState`,
    /// because most of their own callers only hold an immutable borrow -- can
    /// update it without every one of those callers needing to start passing a
    /// mutable borrow through instead.
    pub(in crate::gui::editor) last_pushed_scratch: RefCell<PushedScratch>,
    /// Cache for [`design_material_options`]'s result -- see
    /// [`Self::material_combo_options`], the method that reads/fills it. Same
    /// `RefCell`-through-`&self` discipline as `last_pushed_scratch` just above, and
    /// the same reason: `view::refresh_design_settings` only ever receives
    /// `&EditorState`.
    pub(in crate::gui::editor) material_combo_cache: RefCell<MaterialComboCache>,
    /// Whether this is a REAL design -- created (New Design, any template), loaded
    /// (Load Selected) or opened (Open/Open Recent/the startup restore) --
    /// rather than [`Self::fresh`]'s startup placeholder. Mirrored into
    /// `EditorModel.has_design` by `view::push_has_design`, which gates the
    /// empty-state card grid: a real design with zero tiers must stay visible.
    pub(in crate::gui::editor) has_design: bool,
}

impl EditorState {
    /// A brand-new design: a generously sized cylindrical preform and an empty
    /// schedule, matching the "New" button's job -- start from something that already
    /// renders as a real, closed stone rather than an empty viewport (see
    /// [`EditorSession::fresh`], whose history uses the longer
    /// [`ANGLE_NUDGE_COALESCE_WINDOW`] so an unhurried scroll-wheel nudge still merges
    /// into one undo step; `setup_inline_set_angle_callback`'s no-op branch ends a run
    /// explicitly). Used as the startup placeholder, so [`Self::has_design`] is
    /// `false` here.
    pub(in crate::gui::editor) fn fresh() -> Self {
        // The startup placeholder is not a design the user asked for.
        Self::wrapping(EditorSession::fresh(), false)
    }

    /// A brand-new design from the New Design dialog's full [`FreshDesignSpec`]
    /// (preform, gear, symmetry, mirror, starting material), seeded with template
    /// `template_index`'s starting tiers (`0` is "Empty") -- see
    /// [`EditorSession::from_template`]. Otherwise identical to [`Self::fresh`]:
    /// empty `History`, no printed proportions, no pending work.
    pub(in crate::gui::editor) fn fresh_from_template(
        spec: FreshDesignSpec,
        template_index: i32,
    ) -> Self {
        Self::wrapping(EditorSession::from_template(spec, template_index), true)
    }

    /// `session` with every desktop-only field at its "nothing pending" default.
    fn wrapping(session: EditorSession, has_design: bool) -> Self {
        Self {
            session,
            printed_proportions: None,
            design_epoch: Arc::new(AtomicU64::new(0)),
            pending_unsaved_action: None,
            after_save: None,
            deep_solve: None,
            optimize: None,
            pending_optimize: Arc::new(Mutex::new(None)),
            deep_solve_result_generation: None,
            asc_filename: None,
            original_asc_text: None,
            pending_gear_remap: None,
            pending_retarget: None,
            source_entry_id: None,
            used_placeholder: false,
            file_extras: DesignFileExtras::default(),
            last_pushed_scratch: RefCell::new(PushedScratch::default()),
            material_combo_cache: RefCell::new(MaterialComboCache::default()),
            has_design,
        }
    }

    /// Replaces `self` wholesale with `replacement` -- a brand-new `New`/`Load
    /// Selected`/`Open` state -- while keeping THIS state's own
    /// `generation` counter alive and bumping it, instead of letting
    /// `replacement` bring its own fresh `Arc::new(AtomicU64::new(0))` (as its
    /// constructor -- [`Self::fresh`]/[`Self::fresh_from_template`], or a hand-built
    /// literal -- otherwise would).
    ///
    /// Reusing (and bumping) the SAME `Arc` across the replacement, instead of
    /// letting `replacement` bring its own fresh `Arc::new(AtomicU64::new(0))`,
    /// keeps every staleness check a background closure dispatched BEFORE the
    /// replacement (Deep Solve, Optimize, the debounced auto-solve) already
    /// captured valid: that closure keeps comparing against this SAME `Arc`, so
    /// it observes the bump immediately. A closure comparing against a
    /// replacement-owned `Arc` instead would keep watching a value nothing ever
    /// increments again -- so a search started against design A could complete,
    /// report, and even be applied against design B. The
    /// replacement still reads as clean immediately afterward -- `saved_generation`
    /// is set to match the just-bumped value, not left at whatever the literal
    /// happened to write, so `is_dirty` is `false` right away, matching a freshly
    /// loaded/created design's actual state.
    pub(in crate::gui::editor) fn replace_wholesale(&mut self, mut replacement: Self) {
        // Cancels any Deep Solve/Optimize run still in flight for the design
        // being replaced: dropping the OLD `self.deep_solve`/
        // `self.optimize` handle below (via `*self = replacement`) alone does
        // NOT stop the search -- neither handle cancels itself on drop, so an
        // abandoned run kept burning CPU on its own worker thread for as long
        // as it took to finish (up to several minutes for Deep Solve, see
        // `deep_solve`'s own module doc comment) with no way for the cutter to
        // start a fresh one against the NEW design in the meantime. Calling
        // the real, checkpoint-based `cancel()` here -- the same one the
        // Deep-Solve-Cancel/Optimize-Cancel buttons call -- stops the search
        // itself within roughly the same single-digit-millisecond to
        // few-second latency those buttons document, and lets the abandoned
        // run's own completion handler (which already checks
        // `design_epoch`/`RunProvenance::design_replaced`) arrive promptly
        // instead of stalling behind a long-running search that had no reason
        // to keep going. This only stops the WORK; the caller-visible
        // `editor_deep_solve_running`/`editor_optimize_running` busy flags are
        // a separate `EditorState` has no handle to, reset by
        // `callbacks::solve_actions::clear_analysis_results` at every one of
        // this method's own call sites instead.
        if let Some(handle) = self.deep_solve.take() {
            handle.cancel();
        }
        if let Some(handle) = self.optimize.take() {
            handle.cancel();
        }
        replacement.session.continue_generation_from(&self.session);
        // Carried and bumped exactly like `generation` directly above, and for the
        // same reason -- see [`Self::design_epoch`]'s own doc comment for what the
        // second counter buys that `generation` alone cannot. Bumping it HERE, in
        // the one method every replacement path goes through, is what makes it
        // cover New, Load Selected, Open and Open plain `.asc` uniformly
        // instead of relying on four call sites each remembering to do it.
        replacement.design_epoch = Arc::clone(&self.design_epoch);
        replacement
            .design_epoch
            .fetch_add(1, AtomicOrdering::Relaxed);
        *self = replacement;
    }
}

impl Deref for EditorState {
    type Target = EditorSession;

    fn deref(&self) -> &EditorSession {
        &self.session
    }
}

impl DerefMut for EditorState {
    fn deref_mut(&mut self) -> &mut EditorSession {
        &mut self.session
    }
}
