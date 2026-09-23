//! [`EditorState`] -- the editor's live state (a design plus its undo/redo history) --
//! and the pure view-model helpers that read a [`Design`] into the strings/flags
//! `EditorView` (`types.slint`'s `EditorTierItem`) and the validation banner need. See
//! this group's `mod.rs` doc comment for the "`History` is the only thing that mutates
//! `Design`" rule [`EditorState::apply`]/[`apply_coalescing`](EditorState::apply_coalescing)/
//! [`undo`](EditorState::undo)/[`redo`](EditorState::redo)/
//! [`apply_optimize_outcome`](EditorState::apply_optimize_outcome) exist to uphold --
//! that top-level doc comment predates `apply_coalescing` (the angle-nudge coalescing
//! path added alongside it) and still says "four functions"; this one is the fifth,
//! added the same way and under the same rule.

use super::{auto_solve, material_lookup::EditorMaterialLookup};
use crate::{AngleItem, EditorModel, EditorTierItem, GearRemapRow, IndexChipItem, MainWindow};
use indicatrix::{
    geometry::{
        GpuFacetPlane,
        meet_solver::{
            Block, MeetConstraint, MeetNameResolver, SolveStrategy, SolvedTier, TokenResolution,
            classify_blocks,
        },
        stone_metrics::{SolidMetrics, SolidStatus, build_solid_mesh, measure_solid},
    },
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    ConstraintTier, Design, DesignSolveError, Edit, EditError, FreshDesignSpec, History,
    MaterialSelection, MissingAnchor, OptimizeOutcome, OrbitUnit, PreformSpec, RemapRounding, Risk,
    TierTarget, degenerate_suspects, remap_ratio, tier_margin_deg, windowing_risk,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
};

/// The gear combo's fixed pill choices, before the combo's own trailing "Custom"
/// entry -- shared by the design settings panel's gear control and the new-design
/// dialog (`super::loading::parse_new_design_form`) so both present the same list.
pub(super) const GEAR_PRESETS: [i32; 6] = [96, 80, 77, 72, 64, 120];

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

/// Whether the anchor explainer's "Don't show again" has been persisted.
/// Duplicated in miniature from `native_io::confirm_is_suppressed`'s own
/// `AppSettings::is_confirm_suppressed` pattern -- that function is private to
/// `native_io.rs`, which does not expose a public accessor for it, so it is
/// not callable from here.
fn anchor_explainer_is_suppressed() -> bool {
    let settings_path = crate::settings::store::default_settings_path();
    crate::settings::store::load_or_default(&settings_path)
        .settings
        .is_confirm_suppressed(ANCHOR_EXPLAINER_SUPPRESS_KEY)
}

/// Persists the anchor explainer's "Don't show again" -- the write half of
/// [`anchor_explainer_is_suppressed`]'s pattern.
pub(super) fn anchor_explainer_suppress_permanently() {
    let settings_path = crate::settings::store::default_settings_path();
    let mut file = crate::settings::store::load_or_default(&settings_path);
    file.settings
        .suppress_confirm(ANCHOR_EXPLAINER_SUPPRESS_KEY);
    let _ = crate::settings::store::save(&settings_path, &file);
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
pub(super) fn should_open_anchor_explainer(design_has_missing_anchor: bool) -> bool {
    if !design_has_missing_anchor {
        return false;
    }
    if ANCHOR_EXPLAINER_DECIDED_THIS_SESSION.with(std::cell::Cell::get) {
        return false;
    }
    ANCHOR_EXPLAINER_DECIDED_THIS_SESSION.with(|decided| decided.set(true));
    !anchor_explainer_is_suppressed()
}

/// The coalescing window every [`EditorState`]-owned
/// [`History`] is built with (via [`History::with_coalesce_window`]) instead of
/// [`History::new`]'s crate-wide 500ms default -- long enough that a
/// deliberate, unhurried scroll-wheel angle nudge (ticks slower than 500ms
/// apart) still merges into one undo step. `setup_inline_set_angle_callback`
/// (`callbacks::tier_actions`) explicitly ends a run early with
/// [`History::end_coalesce_run`] on a bit-identical no-op commit, so widening
/// this window does not also widen how long a genuinely finished interaction
/// keeps merging in the (rarer) case that boundary catches.
pub(super) const ANGLE_NUDGE_COALESCE_WINDOW: std::time::Duration =
    std::time::Duration::from_millis(1500);

/// A gear-change the design settings panel has previewed but not yet confirmed --
/// `setup_gear_apply_callback` fills this in and
/// `setup_gear_remap_confirm_callback`/`setup_gear_remap_cancel_callback` are the only
/// two consumers (apply, or discard). `symmetry_order`/`mirror` are the design's own
/// current values at preview time, carried here so confirming needs no second read of
/// `design.meta`.
pub(super) struct PendingGearRemap {
    pub(super) from_gear: i32,
    pub(super) to_gear: i32,
    pub(super) symmetry_order: u32,
    pub(super) mirror: bool,
    pub(super) rounding: RemapRounding,
}

/// A destructive action (New/Load Selected/Open Native) that was about to replace
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
/// (the library selection, a freshly-shown native-file picker) fresh at resume time,
/// which is exactly what running them again from scratch means.
pub(super) enum PendingUnsavedAction {
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
    /// Resume by re-running "Open Native" (which shows the native-file picker again) --
    /// see `gui::editor::native_io::do_open_native`.
    OpenNative,
}

/// The editor's live state: a design plus the undo/redo history over it. See this
/// group's `mod.rs` doc comment for why [`Self::apply`]/[`Self::undo`]/[`Self::redo`]
/// are the only three functions allowed to touch both fields at once.
///
/// Shared via `Rc<RefCell<..>>`, not this crate's usual `Arc<Mutex<..>>`: every
/// callback that touches this state is a Slint callback, which only ever runs on the
/// UI/event-loop thread -- unlike `RenderContext`, which the render thread also reads
/// every frame and genuinely needs a lock for. A real lock here would buy no
/// correctness while inviting clippy's `significant_drop_tightening` lint on every
/// callback holding the guard across a `refresh_all` call.
pub(super) struct EditorState {
    pub(super) design: Design,
    pub(super) history: History,
    /// This design's printed proportions (`Vol/W^3`, `L/W`, `C/W`, `P/W`, `H/W`), when
    /// loaded from a catalogue entry that has them -- Deep Solve's external
    /// verification targets. `None` for a brand-new design or one loaded with those
    /// columns unpopulated; either way Deep Solve must be disabled rather than run
    /// against an unmeasurable target.
    pub(super) printed_proportions:
        Option<indicatrix::geometry::stone_metrics::ExternalProportions>,
    /// Bumped by every successful [`Self::apply`]/[`Self::undo`]/[`Self::redo`].
    /// `Arc<AtomicU64>` so `setup_deep_solve_callback` can clone the counter into a
    /// background thread: a deep solve's completion handler uses it to notice the
    /// design changed mid-search without capturing this non-`Send`
    /// `Rc<RefCell<EditorState>>`-wrapped state directly.
    pub(super) generation: Arc<AtomicU64>,
    /// Bumped ONLY by [`Self::replace_wholesale`] -- i.e. exactly when New / Load
    /// Selected / Open Native swap in a different design, never by an ordinary
    /// edit.
    ///
    /// [`Self::generation`] cannot answer this on its own: it counts edits AND
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
    pub(super) design_epoch: Arc<AtomicU64>,
    /// [`Self::generation`]'s value as of the last successful save (`native_io::
    /// setup_save_native_callback`) OR the moment this particular design was
    /// loaded/created (`fresh`/`fresh_from_spec`/a "Load Selected"/"Open Native"
    /// replacement all start a design at `saved_generation == 0`, matching a brand
    /// new `generation`) -- see [`Self::is_dirty`], the comparison this exists for.
    ///
    /// Compared against the live `generation` counter rather than against `History`'s
    /// own state directly: `History` (`indicatrix_cut_core::edit::History`) exposes no
    /// position/length accessor at all (only `can_undo`/`can_redo`/`peek_undo`/
    /// `peek_redo`), so there is no more precise "how far in" signal to read from
    /// outside that crate. `generation` itself only moves through
    /// [`Self::apply`]/[`Self::apply_coalescing`]/[`Self::undo`]/[`Self::redo`]/
    /// [`Self::apply_optimize_outcome`] -- every one of them a real edit (or, for
    /// undo/redo, an edit-equivalent content change) -- never merely by a solve
    /// (`Design::solve`/`status`/`measure` never touch it), so this is an honest
    /// "has anything changed since the last save" signal for the overwhelming
    /// majority of sessions. Its one known blind spot: undoing back to exactly the
    /// content that was on disk still reads as dirty, since `generation` counts
    /// every step taken rather than net content equality -- accepted rather than
    /// chasing a `Design: PartialEq` snapshot comparison instead, which would need
    /// re-cloning and re-comparing the whole `Design` on every refresh just to
    /// close that one edge case.
    pub(super) saved_generation: u64,
    /// See [`PendingUnsavedAction`]. `None` whenever the Save/Discard/Cancel dialog
    /// is closed (the common state).
    pub(super) pending_unsaved_action: Option<PendingUnsavedAction>,
    /// The in-flight Deep Solve's handle, so `setup_deep_solve_cancel_callback` can
    /// reach it -- `None` when none has ever run. Whether one is CURRENTLY running is
    /// tracked by the `editor_deep_solve_running` Slint property, not by this being
    /// `Some`: the completion callback can't clear this field itself, so a finished
    /// run's handle just sits here until overwritten by the next one.
    pub(super) deep_solve: Option<super::deep_solve::DeepSolveHandle>,
    /// The in-flight Optimize run's handle -- same role as `deep_solve` above.
    pub(super) optimize: Option<super::optimize_solve::OptimizeSolveHandle>,
    /// The most recent COMPLETED or CANCELLED Optimize run's result, held here (never
    /// auto-applied) until `setup_optimize_apply_callback` commits it via
    /// [`Self::apply_optimize_outcome`], or a fresh run supersedes it. Paired with the
    /// design `generation` the search ran against, so an Apply click after the design
    /// has since changed is refused rather than reinterpreting stale tier indices.
    ///
    /// `Arc<Mutex<..>>` for the same non-`Send` reason `generation` is atomic --
    /// Optimize's completion handler runs on a worker thread -- but a `Mutex` here
    /// since the payload (a whole [`OptimizeOutcome`]) isn't atomically representable.
    pub(super) pending_optimize: Arc<Mutex<Option<(OptimizeOutcome, u64)>>>,
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
    pub(super) deep_solve_result_generation: Option<u64>,
    /// The paired `.asc`'s bare file name and exact original text, when this design's
    /// schedule came from a real `.asc` file on disk (a catalogue attachment, or a
    /// previous native save/open). `None` for a brand-new design or one reconstructed
    /// from the angle-table placeholder. Fed to [`indicatrix_cut_core::save_paired`]'s
    /// `original_asc_text` parameter, which lets a native Save leave the `.asc` half
    /// byte-for-byte untouched instead of regenerating it.
    pub(super) asc_filename: Option<String>,
    pub(super) original_asc_text: Option<String>,
    /// The catalogue row this design belongs to, once it has one -- see the
    /// "full round trip" design decision below.
    ///
    /// Set when Load Selected opens a LOCAL row, and by the first Save Native of a
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
    pub(super) source_entry_id: Option<i64>,
    /// Whether `design`'s masts came from `loading::LoadedDesign`'s angle-table
    /// reconstruction fallback -- no attached `.asc` was found, so every mast is a
    /// fabricated `0.0`. Lets Save Native and Export stamp
    /// `indicatrix_formats::asc::mark_reconstructed`, so a file that looks like a
    /// real cut instruction but is not says so in its own header. `false` for every
    /// other construction path.
    pub(super) used_placeholder: bool,
    /// See [`PendingGearRemap`]. `None` whenever the gear remap confirmation panel is
    /// closed (the common state).
    pub(super) pending_gear_remap: Option<PendingGearRemap>,
    /// The last-built "Retarget for material" proposal, paired with the design
    /// `generation` it was built against -- the same stale-result shape
    /// `pending_optimize` uses, simplified to a plain `Option` since nothing here
    /// completes on a background thread. Cleared by `setup_retarget_apply_callback`
    /// (after applying) and `setup_retarget_close_callback` (on cancel).
    pub(super) pending_retarget: Option<(super::retarget::RetargetProposal, u64)>,
    /// The tier-list row indices currently Ctrl+click-toggled into a multi-select
    /// group, for the angle-nudge batch (`setup_nudge_angle_callback`): nudging any
    /// ONE of these while at least two are selected moves all of them together, as
    /// one undoable [`Edit::RetargetAngles`] (see that variant's own doc comment --
    /// this reuses it as-is rather than adding a new `Edit::Batch`, since it's
    /// already exactly "several tiers' angle changes, one undo step, exact per-tier
    /// inverse"). Purely a transient UI selection, never itself part of `Design` or
    /// `History`: [`Self::apply`]/[`Self::undo`]/[`Self::redo`] prune it back to
    /// valid indices after every edit (see their own bodies) rather than leaving a
    /// stale index that outlived the tier it once named. Cleared to empty by
    /// `EditorState::fresh`/`fresh_from_spec`/a catalogue load, matching every other
    /// per-design transient field here.
    pub(super) multi_selected: BTreeSet<usize>,
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
    pub(super) last_pushed_scratch: RefCell<PushedScratch>,
    /// Cache for [`design_material_options`]'s result -- see
    /// [`Self::material_combo_options`], the method that reads/fills it. Same
    /// `RefCell`-through-`&self` discipline as `last_pushed_scratch` just above, and
    /// the same reason: `view::refresh_design_settings` only ever receives
    /// `&EditorState`.
    pub(super) material_combo_cache: RefCell<MaterialComboCache>,
}

/// See [`EditorState::last_pushed_scratch`]. Every field is `None` (or, for
/// `girdle_diameter_mm`, [`GirdleDiameterPush::Unobserved`]) until the first push
/// observes it, so the very first refresh after New/Load always reports every
/// group changed (a correct, if slightly redundant, initial push).
#[derive(Default)]
pub(super) struct PushedScratch {
    material: Option<MaterialSelection>,
    gear_teeth: Option<i32>,
    symmetry: Option<(u32, bool)>,
    preform: Option<PreformSpec>,
    girdle_diameter_mm: GirdleDiameterPush,
    /// `(headers, footnotes, gear_reference_angle)` -- see
    /// [`ScratchDelta::meta`]'s own doc comment.
    meta: Option<(Vec<String>, Vec<String>, f64)>,
}

/// [`EditorState::material_combo_cache`]'s contents: [`design_material_options`]'s
/// last result, plus the custom-material name list (in `custom_materials` order) it
/// was built from.
///
/// `view::refresh_design_settings` only calls
/// `design_material_options(&ctx.custom_materials)` again when the combo's actual
/// contents change -- a custom material saved, deleted, or renamed in the
/// material editor dialog -- rather than on every editor refresh: every other
/// refresh (an angle nudge, a solve, a tier edit) would otherwise rebuild the same
/// now-33-plus-customs-entry `Vec<String>` for nothing. `signature` is only the
/// NAME list, not the full `GemMaterial`: `design_material_options` lists names
/// only (an RI/absorption edit to an existing custom material changes no name in
/// the list, so it cannot change this combo's contents either).
///
/// `signature` is `None` until the first build, not a plain `Vec` default: with no
/// custom materials in the vault the incoming name list is empty, which would
/// equal an empty default signature, so the cache would never rebuild and the
/// combo would receive an EMPTY model -- Slint's `ComboBox` then clears
/// `current-value` to `""`, which shows as a blank Material box for every loaded
/// design.
#[derive(Default)]
pub(super) struct MaterialComboCache {
    signature: Option<Vec<String>>,
    options: Vec<String>,
}

/// [`PushedScratch::girdle_diameter_mm`]'s own value -- NOT a plain
/// `Option<Option<f64>>`, since the design's own `girdle_diameter_mm` is already an
/// `Option<f64>` (unset vs a real dimension): nesting it in another `Option` would
/// conflate that "unset" state with "this push has never been observed yet", the
/// same distinction every sibling field in [`PushedScratch`] uses its own outer
/// `Option` for. This makes the two levels explicit instead.
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum GirdleDiameterPush {
    /// No push has been recorded yet -- the very first refresh after New/Load
    /// always reports `girdle` changed (see [`PushedScratch`]'s own doc comment).
    #[default]
    Unobserved,
    /// The last-pushed value; `None` when the design itself has no girdle
    /// diameter set.
    Observed(Option<f64>),
}

/// Which of [`PushedScratch`]'s groups actually changed since the last push,
/// returned by [`EditorState::record_scratch_push`] -- five independent flags
/// rather than one "anything changed" bit, because a design-settings-only
/// change (say, Apply Gear) must
/// never also blank an in-progress, unrelated edit sitting in the Preform tab's
/// Half-Width field, and vice versa. `view::refresh_design_settings` gates the
/// material/gear/symmetry pushes on their own flags; the preform+yield push in
/// `view::refresh_editor_panel`/`view::push_stale_content` gates on `preform`/
/// `girdle`/`material` the same way.
pub(super) struct ScratchDelta {
    pub(super) material: bool,
    pub(super) gear: bool,
    pub(super) symmetry: bool,
    pub(super) preform: bool,
    pub(super) girdle: bool,
    /// Whether `design.meta`'s headers/footnotes/gear-reference-
    /// angle changed since the last push -- gates `EditorModel.design_title`/
    /// `design_extra_headers`/`design_footnotes`/`design_gear_reference_angle`'s
    /// own reseed in `view::refresh_design_settings`, same "don't blank an
    /// in-progress, unrelated edit" reasoning as every other field here.
    pub(super) meta: bool,
}

impl EditorState {
    /// A brand-new design: a generously sized cylindrical preform and an empty
    /// schedule, matching the "New" button's job -- start from something that already
    /// renders as a real, closed stone rather than an empty viewport.
    pub(super) fn fresh() -> Self {
        let preform = indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5);
        Self {
            design: Design::fresh(preform, 96, 8, 1.54),
            // A longer, per-instance coalescing window
            // (`History::with_coalesce_window`, not the crate-wide 500ms
            // `History::new()` default) so a deliberate, unhurried scroll-wheel
            // angle nudge (ticks slower than 500ms apart) still merges into one
            // undo step instead of costing one Ctrl+Z per tick. See
            // `setup_inline_set_angle_callback`'s no-op branch (`callbacks::
            // tier_actions`) for this history's own explicit `end_coalesce_run`
            // boundary.
            history: History::with_coalesce_window(ANGLE_NUDGE_COALESCE_WINDOW),
            printed_proportions: None,
            generation: Arc::new(AtomicU64::new(0)),
            design_epoch: Arc::new(AtomicU64::new(0)),
            saved_generation: 0,
            pending_unsaved_action: None,
            deep_solve: None,
            optimize: None,
            pending_optimize: Arc::new(Mutex::new(None)),
            deep_solve_result_generation: None,
            asc_filename: None,
            original_asc_text: None,
            pending_gear_remap: None,
            pending_retarget: None,
            multi_selected: BTreeSet::new(),
            source_entry_id: None,
            used_placeholder: false,
            last_pushed_scratch: RefCell::new(PushedScratch::default()),
            material_combo_cache: RefCell::new(MaterialComboCache::default()),
        }
    }

    /// A brand-new design from the New Design dialog's full [`FreshDesignSpec`]
    /// (preform, gear, symmetry, mirror, starting material). Otherwise identical to
    /// [`Self::fresh`]: empty `History`, no printed proportions, no pending work.
    pub(super) fn fresh_from_spec(spec: FreshDesignSpec) -> Self {
        Self {
            design: Design::fresh_from_spec(spec),
            // See `Self::fresh`'s matching comment on the coalescing window.
            history: History::with_coalesce_window(ANGLE_NUDGE_COALESCE_WINDOW),
            printed_proportions: None,
            generation: Arc::new(AtomicU64::new(0)),
            design_epoch: Arc::new(AtomicU64::new(0)),
            saved_generation: 0,
            pending_unsaved_action: None,
            deep_solve: None,
            optimize: None,
            pending_optimize: Arc::new(Mutex::new(None)),
            deep_solve_result_generation: None,
            asc_filename: None,
            original_asc_text: None,
            pending_gear_remap: None,
            pending_retarget: None,
            multi_selected: BTreeSet::new(),
            source_entry_id: None,
            used_placeholder: false,
            last_pushed_scratch: RefCell::new(PushedScratch::default()),
            material_combo_cache: RefCell::new(MaterialComboCache::default()),
        }
    }

    /// Replaces `self` wholesale with `replacement` -- a brand-new `New`/`Load
    /// Selected`/`Open Native` state -- while keeping THIS state's own
    /// `generation` counter alive and bumping it, instead of letting
    /// `replacement` bring its own fresh `Arc::new(AtomicU64::new(0))` (as its
    /// constructor -- [`Self::fresh`]/[`Self::fresh_from_spec`], or a hand-built
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
    pub(super) fn replace_wholesale(&mut self, mut replacement: Self) {
        replacement.generation = Arc::clone(&self.generation);
        let now = replacement.generation.fetch_add(1, AtomicOrdering::Relaxed) + 1;
        replacement.saved_generation = now;
        // Carried and bumped exactly like `generation` directly above, and for the
        // same reason -- see [`Self::design_epoch`]'s own doc comment for what the
        // second counter buys that `generation` alone cannot. Bumping it HERE, in
        // the one method every replacement path goes through, is what makes it
        // cover New, Load Selected, Open Native and Open plain `.asc` uniformly
        // instead of relying on four call sites each remembering to do it.
        replacement.design_epoch = Arc::clone(&self.design_epoch);
        replacement
            .design_epoch
            .fetch_add(1, AtomicOrdering::Relaxed);
        *self = replacement;
    }

    /// Applies `edit` through [`History::apply`] -- the only way this group mutates
    /// `design`. See this group's `mod.rs` doc comment.
    pub(super) fn apply(&mut self, edit: Edit) -> Result<(), EditError> {
        let Self {
            design, history, ..
        } = self;
        let outcome = history.apply(design, edit);
        if outcome.is_ok() {
            self.generation.fetch_add(1, AtomicOrdering::Relaxed);
            self.prune_multi_selected();
        }
        outcome
    }

    /// Like [`Self::apply`], but through [`History::apply_coalescing`] instead --
    /// what the angle-nudge keyboard/wheel handlers use so several nudges typed in
    /// quick succession (`key`, matched against `History`'s own 500ms window) collapse
    /// into one undo step. See `angle_nudge_coalesce_key` for how the editor derives
    /// `key` from the nudge's target tier(s).
    ///
    /// # Errors
    ///
    /// Propagates [`History::apply_coalescing`]'s error verbatim.
    pub(super) fn apply_coalescing(&mut self, edit: Edit, key: u64) -> Result<(), EditError> {
        let Self {
            design, history, ..
        } = self;
        let outcome = history.apply_coalescing(design, edit, key, std::time::Instant::now());
        if outcome.is_ok() {
            self.generation.fetch_add(1, AtomicOrdering::Relaxed);
            self.prune_multi_selected();
        }
        outcome
    }

    /// Undoes through [`History::undo`] -- see [`Self::apply`].
    ///
    /// # Errors
    ///
    /// Propagates [`History::undo`]'s error verbatim (a failed replay of the recorded
    /// inverse edit) -- the caller (`gui::editor::callbacks::tier_actions::
    /// setup_undo_callback`) surfaces this via a toast rather than unwrapping/panicking,
    /// since `History::undo` returns `Err` here instead of panicking.
    pub(super) fn undo(&mut self) -> Result<bool, EditError> {
        let Self {
            design, history, ..
        } = self;
        let undone = history.undo(design)?;
        if undone {
            self.generation.fetch_add(1, AtomicOrdering::Relaxed);
            self.prune_multi_selected();
        }
        Ok(undone)
    }

    /// Redoes through [`History::redo`] -- see [`Self::undo`].
    ///
    /// # Errors
    ///
    /// Same as [`Self::undo`], symmetrically for [`History::redo`].
    pub(super) fn redo(&mut self) -> Result<bool, EditError> {
        let Self {
            design, history, ..
        } = self;
        let redone = history.redo(design)?;
        if redone {
            self.generation.fetch_add(1, AtomicOrdering::Relaxed);
            self.prune_multi_selected();
        }
        Ok(redone)
    }

    /// Drops any [`Self::multi_selected`] index that no longer names a real tier --
    /// called after every successful [`Self::apply`]/[`Self::apply_coalescing`]/
    /// [`Self::undo`]/[`Self::redo`], since any of those can change `design.tiers`'
    /// length (an `AddTier`/`RemoveTier`, or undoing/redoing one).
    fn prune_multi_selected(&mut self) {
        let tier_count = self.design.tiers.len();
        self.multi_selected.retain(|&index| index < tier_count);
    }

    /// Applies `outcome`'s [`indicatrix_cut_core::AngleChange`]s through
    /// [`indicatrix_cut_core::apply_optimize_outcome`] -- the one path that turns an
    /// Optimize result into real, undoable edits (one [`Edit::ModifyTier`] per changed
    /// tier, via `History`). The fourth and last function allowed to touch `design`
    /// and `history` together.
    ///
    /// Bumps `generation` for `Ok(applied)` iff `applied > 0`, matching
    /// [`Self::apply`]/[`Self::undo`]/[`Self::redo`] -- but also on `Err`, unlike
    /// them: `apply_optimize_outcome` stops at the first tier index that no longer
    /// names a real tier, but every change before that point already went through
    /// `History::apply` for real, and its `Result` gives no way to learn "how many"
    /// on `Err`. Bumping unconditionally errs toward over-invalidating rather than
    /// silently treating a partially-edited design as unchanged.
    pub(super) fn apply_optimize_outcome(
        &mut self,
        outcome: &OptimizeOutcome,
    ) -> Result<usize, EditError> {
        let Self {
            design, history, ..
        } = self;
        let result = indicatrix_cut_core::apply_optimize_outcome(history, design, outcome);
        match &result {
            Ok(0) => {}
            Ok(_) | Err(_) => {
                self.generation.fetch_add(1, AtomicOrdering::Relaxed);
            }
        }
        result
    }

    /// Whether `design` has changed since the last successful save/load/new -- see
    /// [`Self::saved_generation`]'s own doc comment for exactly what this compares
    /// and why. Read by the New/Load Selected/Open Native callbacks and window-close
    /// handling before they replace or discard this state, and by
    /// `EditorModel.recompute_dirty`'s handler (`gui::editor::native_io::
    /// setup_dirty_tracking`) to keep the status strip's "Unsaved" marker and the
    /// window title's leading marker live.
    #[must_use]
    pub(super) fn is_dirty(&self) -> bool {
        self.generation.load(AtomicOrdering::Relaxed) != self.saved_generation
    }

    /// Compares `self.design`'s current settings/preform/yield-relevant fields
    /// against the [`PushedScratch`] snapshot recorded the last time this ran,
    /// records a fresh snapshot, and returns which groups actually changed. See
    /// [`ScratchDelta`]'s own doc comment for why each group is independent.
    /// Called exactly once per `view::refresh_editor_panel`/`view::
    /// push_stale_content` invocation (never from inside a function those two
    /// call, or a group could be compared against a snapshot already
    /// overwritten by an earlier group in the same refresh).
    pub(super) fn record_scratch_push(&self) -> ScratchDelta {
        let design = &self.design;
        let mut cache = self.last_pushed_scratch.borrow_mut();
        let meta_now = (
            design.meta.headers.clone(),
            design.meta.footnotes.clone(),
            design.meta.gear_reference_angle,
        );
        let delta = ScratchDelta {
            material: cache.material.as_ref() != Some(&design.material),
            gear: cache.gear_teeth != Some(design.meta.gear_teeth),
            symmetry: cache.symmetry != Some((design.meta.symmetry_order, design.meta.mirror)),
            preform: cache.preform != Some(design.preform),
            girdle: cache.girdle_diameter_mm
                != GirdleDiameterPush::Observed(design.girdle_diameter_mm),
            meta: cache.meta.as_ref() != Some(&meta_now),
        };
        *cache = PushedScratch {
            material: Some(design.material.clone()),
            gear_teeth: Some(design.meta.gear_teeth),
            symmetry: Some((design.meta.symmetry_order, design.meta.mirror)),
            preform: Some(design.preform),
            girdle_diameter_mm: GirdleDiameterPush::Observed(design.girdle_diameter_mm),
            meta: Some(meta_now),
        };
        delta
    }

    /// [`design_material_options`], rebuilt only when `custom`'s own name list has
    /// changed since the last call -- see [`MaterialComboCache`]'s own doc comment.
    /// `view::refresh_design_settings` calls this every refresh; only a save/delete/
    /// rename in the material editor dialog (which changes `custom`'s names) actually
    /// pays for a rebuild.
    pub(super) fn material_combo_options(&self, custom: &[GemMaterial]) -> Vec<String> {
        let signature: Vec<String> = custom.iter().map(|m| m.name.clone()).collect();
        let mut cache = self.material_combo_cache.borrow_mut();
        if cache.signature.as_ref() != Some(&signature) {
            cache.options = design_material_options(custom);
            cache.signature = Some(signature);
        }
        cache.options.clone()
    }
}

/// Splits a tier's [`MeetConstraint`] AND its [`TierTarget`] (if any) into the
/// `(constraint_kind, constraint_text)` pair [`EditorTierItem`] carries and the
/// tier-edit form round-trips through its `LineEdit` -- the inverse of
/// `super::loading::parse_tier_form`'s constraint parsing and
/// `super::loading::parse_tier_target`'s target parsing, combined.
///
/// `target` wins whenever it is `Some`: `Design::resolved_meet_tier_inputs`
/// always leaves `constraint` as a `ScaleReference(0.0)` PLACEHOLDER on a
/// target-bearing tier (`indicatrix_cut_core::design::targets`'s module docs),
/// so reading `constraint` directly would show kind `2` with a meaningless
/// "0" rather than the target the cutter actually authored.
fn constraint_kind_and_text(
    constraint: &MeetConstraint,
    target: Option<TierTarget>,
) -> (i32, String) {
    if let Some(target) = target {
        return match target {
            TierTarget::DepthMm(mm) => (3, mm.to_string()),
            TierTarget::GirdleThicknessMm(mm) => (4, mm.to_string()),
            TierTarget::TableWidthMm(mm) => (5, mm.to_string()),
        };
    }
    match constraint {
        MeetConstraint::MeetExisting => (0, String::new()),
        MeetConstraint::MeetNamed(names) => (1, names.join(", ")),
        // Rust's shortest round-trippable `f64` Display, not a fixed decimal count --
        // this feeds back into the form's `LineEdit`, so it must read back exactly
        // what `solve` (or the user) produced.
        MeetConstraint::ScaleReference(value) => (2, value.to_string()),
    }
}

/// The tier table's ANGLE cell text -- always two decimals:
/// a wheel/keyboard nudge accumulates plain `f64` noise (e.g.
/// `-40.300000000000004`), and showing that raw `Display` output read as a
/// cut-off, broken number rather than a rounding artifact. A cutter compares
/// this against a 2-decimal gauge anyway, so truncating the DISPLAY (never the
/// stored value -- the inline editor and the tier-edit form both still read the
/// full, unrounded value) is honest and matches every other angle readout in
/// this app (`view::refresh_design_settings`'s critical-angle text, the margin
/// column, ...).
fn format_angle_cell(angle_deg: f64) -> String {
    format!("{angle_deg:.2}")
}

/// One index-wheel position's display text -- an integer when it lands exactly
/// on a whole tooth (the overwhelming common case), else two decimals. Mirrors
/// [`format_angle_cell`]'s reasoning for the same nudge-noise problem, but
/// integral-aware: `"24"` reads better than `"24.00"` for the ordinary case,
/// while a genuinely fractional index (rare, but real -- see
/// `ManufacturabilityWarning::FractionalIndex`) still shows its non-integral part.
fn format_index_value(value: f64) -> String {
    if value.fract().abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// Builds one [`IndexChipItem`] per entry in `indices`, flagging exactly the
/// occurrences also present in `detached` -- the inspector's per-facet chip row,
/// pushed by `gui::editor::view::selected_tier_chips` as its
/// own `EditorModel.selected_tier_chips`, NOT a field on [`EditorTierItem`] --
/// that struct's `Vec` is built on a background thread for a large design
/// (`gui::editor::auto_solve`) and shipped to the UI thread inside
/// `BackgroundSolveResult`, which must stay `Send`; a `[IndexChipItem]` field
/// there is `ModelRc`-backed (`Rc`, not `Send`) and does not compile.
///
/// A plain epsilon-equality scan against `detached` (not `crate::orbit::edit`'s
/// private, gear-wraparound-aware `same_index`, unreachable from here) is correct
/// for this: both `Vec`s are the SAME tier's own values, `detached` entries are
/// always pushed from `indices` verbatim (`Design::detach_orbit_member`/
/// `detach_all_in_tier`), never independently authored or wrapped, so no
/// wraparound normalization is ever needed to tell them apart -- only real edits
/// (`facet_toggle_detach`'s Rust handler) need the wraparound-aware comparison,
/// and those already go through core's own helpers.
#[must_use]
pub(super) fn index_chip_items(indices: &[f64], detached: &[f64]) -> Vec<IndexChipItem> {
    indices
        .iter()
        .map(|&position| IndexChipItem {
            position: position as f32,
            label: format_index_value(position).into(),
            detached: detached.iter().any(|&d| (d - position).abs() < 1e-9),
        })
        .collect()
}

/// Display text for a tier's `imported_meet` -- what the source `.asc` file's `G`
/// field claimed this facet meets, kept alongside the pinned `constraint` import now
/// writes instead. `""` when there's nothing to adopt, which `EditorView` also uses
/// as its "show the Adopt button" condition.
fn imported_meet_text(imported_meet: Option<&MeetConstraint>) -> String {
    match imported_meet {
        Some(MeetConstraint::MeetExisting) => "meets an unspecified vertex".to_string(),
        Some(MeetConstraint::MeetNamed(names)) => format!("meets {}", names.join(", ")),
        // `Design::from_asc_schedule` never actually stores this variant here, but a
        // future non-exhaustive addition should degrade to "nothing to show," not panic.
        None | Some(MeetConstraint::ScaleReference(_)) => String::new(),
    }
}

/// [`SolveStrategy`]'s human-readable label for the tier list's "SOLVE" column, plus
/// whether it should be flagged uncertain: `LeastSquaresFallback` is an estimate, not
/// vertex-derived, and `Failed` is documented as "should not be trusted" -- both get
/// flagged, `ScaleReference`/`DependencyOrder`/`JointGroup` do not.
/// A short label for `units` (`orbit::orbit_units`'s decomposition of one tier's
/// `indices`) plus whether it should be flagged amber. Empty/`false` for a tier with
/// nothing to link (0 or 1 occurrence).
fn orbit_status_text(units: &[OrbitUnit]) -> (String, bool) {
    if units.iter().all(|u| u.members.len() <= 1) {
        return (String::new(), false);
    }
    match units {
        [one] => {
            if one.is_complete() {
                (format!("orbit x{}", one.members.len()), false)
            } else {
                (
                    format!("{}/{} orbit", one.members.len(), one.expected_len),
                    true,
                )
            }
        }
        // Reports which of the two cases this actually is -- a clean multi-facet
        // fold with one unit short a member, vs a `mixed_fold` where NOTHING
        // resembles a complete orbit -- rather than folding both into one "N
        // orbits" label indistinguishable except by the amber tint.
        // `orbit::mod`'s own corpus-measurement doc comment treats those
        // as different findings (a `partial` occurrence is common and benign;
        // `mixed_fold` -- every unit incomplete -- is "real incoherence").
        many => {
            let incomplete = many.iter().filter(|u| !u.is_complete()).count();
            if incomplete == 0 {
                (format!("{} orbits", many.len()), false)
            } else if incomplete == many.len() {
                ("not symmetric".to_string(), true)
            } else {
                (
                    format!("{} orbits ({incomplete} incomplete)", many.len()),
                    true,
                )
            }
        }
    }
}

/// Previews a proposed Symmetry Order/Mirror change's effect
/// on every tier's orbit BEFORE `setup_apply_symmetry_callback` actually applies
/// it, mirroring the gear-remap path's own dry-run preview (`gear_remap_preview`),
/// which likewise never mutates `design` to compute its summary. Clones
/// `design.meta` and swaps in only the two fields Apply Symmetry can change --
/// never `design` itself -- then counts a tier the same way its own "orbit"
/// table badge would ([`orbit_status_text`]'s second return value), so "N tiers
/// would become incomplete" always agrees with what those rows will show once
/// applied.
#[must_use]
pub(super) fn tiers_incomplete_under_proposed_symmetry(
    design: &Design,
    symmetry_order: u32,
    mirror: bool,
) -> usize {
    let mut proposed_meta = design.meta.clone();
    proposed_meta.symmetry_order = symmetry_order;
    proposed_meta.mirror = mirror;
    design
        .tiers
        .iter()
        .filter(|tier| {
            let units = indicatrix_cut_core::orbit_units(&tier.indices, &proposed_meta);
            orbit_status_text(&units).1
        })
        .count()
}

/// Whether an analysis result
/// stamped with `result_generation` (Deep Solve's [`EditorState::
/// deep_solve_result_generation`], Optimize's `pending_optimize`-stored
/// generation, or any future caller's own equivalent) is stale against
/// `current_generation` -- i.e. the design has moved on since that result was
/// computed. `None` (no result has ever completed) is never stale -- there is
/// nothing to badge yet, not a result "as stale as it gets."
///
/// Pure and unit tested directly: this is the one decision every "Stale: design
/// changed" badge in this app reduces to, whatever Slint property or `ui/
/// components/stale_badge.slint` instance ends up reading it.
#[must_use]
pub(super) const fn result_is_stale(
    result_generation: Option<u64>,
    current_generation: u64,
) -> bool {
    match result_generation {
        Some(g) => g != current_generation,
        None => false,
    }
}

/// Patches [`EditorTierItem::multi_selected`] onto every row in `rows` from
/// `multi_selected` -- the post-pass a full tier-list rebuild ([`tier_items`]/
/// [`tier_items_stale`]) needs to survive with the live multi-select highlight
/// intact, since neither builder itself knows about `EditorState::multi_selected`
/// (both always set the flag to `false`; see their own doc comments). Every call
/// site that replaces the WHOLE `editor_tiers` model applies this immediately
/// afterward: `view::refresh_editor_panel`/`push_stale_content`, and
/// `auto_solve`'s background-solve completion. `setup_toggle_multi_select_callback`
/// is the one exception -- it patches the flag onto an ALREADY-pushed model in
/// place instead, using this same function, since toggling a selection changes
/// nothing about `Design` and must never re-run a full tier-list rebuild.
pub(super) fn apply_multi_selection(rows: &mut [EditorTierItem], multi_selected: &BTreeSet<usize>) {
    for row in rows {
        row.multi_selected =
            usize::try_from(row.index).is_ok_and(|index| multi_selected.contains(&index));
    }
}

/// Pushes `EditorModel.multi_selected_count` -- the tier table's "N selected"
/// header indicator (`editor_tier_table.slint`) -- kept a SEPARATE call from
/// [`apply_multi_selection`] rather than folded into it, since one of that
/// function's call sites (`auto_solve`'s background-solve worker thread) runs off
/// the UI thread and must never touch a Slint global; every caller of THIS
/// function, by contrast, already runs on the UI thread (the two `view::` refresh
/// paths, the toggle/selection-changed callbacks in `callbacks::tier_actions`, and
/// `auto_solve`'s UI-thread completion handler).
pub(super) fn push_multi_selected_count(ui: &MainWindow, count: usize) {
    ui.global::<EditorModel>()
        .set_multi_selected_count(i32::try_from(count).unwrap_or(i32::MAX));
}

/// Pushes `rows` into `EditorModel.tiers`, reusing the existing model via
/// [`slint::Model::set_row_data`] when the row count is unchanged instead of
/// replacing the whole `ModelRc` -- an ordinary edit (`ModifyTier`), an undo/redo
/// that doesn't change the tier count, or a background-solve completion never
/// resizes the list, and Slint only recreates a `for` loop's per-row component
/// tree when the MODEL ITSELF changes identity, not when one row's data does. A
/// wholesale replacement tears down and rebuilds every row's component tree on every
/// refresh, including one mid-inline-edit -- dropping keyboard focus
/// out of an open inline angle edit (`editor_tier_table.slint`'s `TierAngleCell`)
/// on every refresh, which is exactly what the same-length reuse path above avoids.
/// A structural edit that actually changes the tier count
/// (`AddTier`/`RemoveTier`, or undoing/redoing one) still needs a real replacement
/// -- `set_row_data` cannot resize a model -- so that case still replaces the model
/// wholesale.
///
/// Explicitly invokes `EditorModel.recompute_dirty` afterward rather than relying
/// on `editor.slint`'s own `changed tiers => { recompute_dirty(); }` watcher to
/// catch it: that watcher only fires when the `tiers` PROPERTY itself is
/// reassigned (the `set_tiers` branch below), never when `set_row_data` merely
/// mutates the SAME `ModelRc`'s contents in place -- without this explicit call,
/// the dirty/"Unsaved" indicator would stop updating for the common case (an
/// edit that doesn't change the tier count) the moment that branch is taken.
pub(super) fn push_tiers(ui: &MainWindow, rows: Vec<EditorTierItem>) {
    push_rows(&ui.global::<EditorModel>().get_tiers(), rows, |model| {
        ui.global::<EditorModel>().set_tiers(model);
    });
    ui.global::<EditorModel>().invoke_recompute_dirty();
}

/// The general form of [`push_tiers`]'s own in-place-update trick (see that
/// function's own doc comment for the full "why" -- rebuilding a Slint `for`
/// loop's whole component tree on every refresh dropped keyboard focus out of an
/// open inline edit): reuses `current` via [`slint::Model::set_row_data`] when its
/// row count already matches `rows`, calling `set` with a fresh `ModelRc` only
/// when the length actually changed (an add/remove, not an ordinary edit).
///
/// `set` is called ONLY on that replace path, never on the reuse path -- exactly
/// matching [`push_tiers`]'s own original behaviour (see its doc comment on why
/// `EditorModel.tiers`'s reassignment is what fires `editor.slint`'s `changed
/// tiers` watcher, and why reusing `current` in place must not also trigger it
/// again for nothing). A caller pushing a property with no such watcher (every
/// other use below) still benefits: skipping the property write when nothing
/// structural changed is itself the point, whether or not Slint's own property
/// setter would already have elided a same-model reassignment.
///
/// `view::push_stale_content`/
/// `push_manufacturability_and_preform_scratch`/`push_selected_tier_chips`
/// (`view.rs`, not this file) route `EditorModel.cutting_rows`/
/// `manufacturability_warnings`/`manufacturability_warning_tiers`/
/// `selected_tier_chips` through this same generic helper instead of each
/// replacing the model with a brand-new `ModelRc<VecModel<_>>` on every single
/// refresh, even a same-length one -- this is what lets each of
/// those call sites share the identical incremental-update behaviour `push_tiers`
/// already had, without four near-duplicate copies of the same length-check.
///
/// Callers still own deciding WHAT to push (the `Vec<T>` computation itself is
/// unchanged); this only changes HOW it reaches `EditorModel`.
pub(super) fn push_rows<T: Clone + 'static>(
    current: &ModelRc<T>,
    rows: Vec<T>,
    set: impl FnOnce(ModelRc<T>),
) {
    if current.row_count() == rows.len() {
        for (index, row) in rows.into_iter().enumerate() {
            current.set_row_data(index, row);
        }
    } else {
        set(ModelRc::new(VecModel::from(rows)));
    }
}

/// The first name in `names` (a `MeetConstraint::MeetNamed` constraint's typed
/// list) that does not resolve against `design`'s current tiers -- built from the
/// SAME [`MeetNameResolver`] `indicatrix::geometry::meet_solver::solve` itself
/// uses, so a name that would otherwise silently degrade to a dropped token inside
/// the solver (see that module's own doc comment) is instead caught at Save Tier
/// time with a specific, actionable message. `None` when every name resolves to a
/// real tier or is a recognized meet-point word/connective prose
/// (`TokenResolution::Ignorable`).
pub(super) fn first_unresolved_meet_name(design: &Design, names: &[String]) -> Option<String> {
    let inputs = design.meet_tier_inputs();
    let resolver = MeetNameResolver::new(&inputs);
    names
        .iter()
        .find(|name| matches!(resolver.resolve_token(name), TokenResolution::Unresolved))
        .cloned()
}

/// [`EditorTierItem::margin_text`]/`risk_level` for one tier -- pavilion tiers
/// (`tier_angle_deg < 0.0`) read the plain table-only critical-angle margin
/// ([`tier_margin_deg`]/[`windowing_risk`]); crown tiers (`tier_angle_deg >
/// 0.0`) read the crown-window ESTIMATE ([`indicatrix_cut_core::
/// crown_window_margin_deg`]/[`indicatrix_cut_core::crown_windowing_risk`])
/// against `pavilion_partner_deg`, suffixed `" (est.)"` so it is
/// never mistaken for the same table-only certainty a pavilion row's margin
/// carries -- see that function's own doc comment for exactly what the
/// estimate does and does not model. A girdle tier (`tier_angle_deg == 0.0`,
/// or a crown tier when no pavilion angle could be found at all) always reads
/// `("", -1)`, "nothing to show," not a wrong badge. `n_d` is the design's
/// effective refractive index, the same value the design settings panel's
/// RI/critical-angle readouts show.
fn tier_margin_and_risk(
    tier_angle_deg: f64,
    n_d: f64,
    pavilion_partner_deg: Option<f64>,
) -> (String, i32) {
    let risk_level_of = |risk: Risk| match risk {
        Risk::Safe => 0,
        Risk::Marginal => 1,
        Risk::Windows => 2,
    };
    if tier_angle_deg < 0.0 {
        let margin = tier_margin_deg(tier_angle_deg, n_d);
        let risk_level = risk_level_of(windowing_risk(tier_angle_deg, n_d));
        (format!("{margin:+.1}\u{b0}"), risk_level)
    } else if tier_angle_deg > 0.0 {
        let Some(pavilion_deg) = pavilion_partner_deg else {
            return (String::new(), -1);
        };
        let margin =
            indicatrix_cut_core::crown_window_margin_deg(pavilion_deg, tier_angle_deg, n_d);
        let risk_level = risk_level_of(indicatrix_cut_core::crown_windowing_risk(
            pavilion_deg,
            tier_angle_deg,
            n_d,
        ));
        (format!("{margin:+.1}\u{b0} (est.)"), risk_level)
    } else {
        (String::new(), -1)
    }
}

/// The design's own representative crown/pavilion facet angles, in degrees
/// (magnitudes), for the crown-window-estimate margin
/// ([`tier_margin_and_risk`]) and the proportion-verdict angle metrics
/// (`view::push_yield_and_proportions`): the tier whose name contains "main"
/// (case-insensitive) on each side, or -- when no tier is named that way --
/// the tier with the largest magnitude on that side. "Crown Main"/"Pavilion
/// Main" is this crate's own template naming convention
/// ([`ConstraintTier::standard_round_brilliant`] and every
/// [`indicatrix_cut_core::templates::TEMPLATES`] entry), so this reads the
/// real main facet for every template-derived design and falls back to a
/// reasonable guess for a hand-authored one using different names. `None` on
/// either side when the design has no tier on that side at all.
pub(super) fn representative_crown_and_pavilion_angles_deg(
    design: &Design,
) -> (Option<f64>, Option<f64>) {
    let mut crown_main: Option<f64> = None;
    let mut crown_largest: Option<f64> = None;
    let mut pavilion_main: Option<f64> = None;
    let mut pavilion_largest: Option<f64> = None;
    for tier in &design.tiers {
        let angle = tier.angle_deg;
        let is_main = tier.name.to_lowercase().contains("main");
        if angle > 0.0 {
            crown_largest = Some(crown_largest.map_or(angle, |m| angle.max(m)));
            if is_main {
                crown_main = Some(crown_main.map_or(angle, |m| angle.max(m)));
            }
        } else if angle < 0.0 {
            let magnitude = angle.abs();
            pavilion_largest = Some(pavilion_largest.map_or(magnitude, |m| magnitude.max(m)));
            if is_main {
                pavilion_main = Some(pavilion_main.map_or(magnitude, |m| magnitude.max(m)));
            }
        }
    }
    (
        crown_main.or(crown_largest),
        pavilion_main.or(pavilion_largest),
    )
}

const fn strategy_label(strategy: SolveStrategy) -> (&'static str, bool) {
    match strategy {
        SolveStrategy::ScaleReference => ("Scale reference", false),
        SolveStrategy::DependencyOrder => ("Dependency order", false),
        SolveStrategy::JointGroup => ("Joint group", false),
        SolveStrategy::LeastSquaresFallback => ("Least-squares est.", true),
        SolveStrategy::Failed => ("FAILED (untrusted)", true),
    }
}

/// Maps every tier index [`MissingAnchor::block_details`] names to the exact
/// [`Block`] it belongs to -- lets [`tier_items`] mark exactly the tiers
/// responsible for a [`MissingAnchor`] failure (and give each one its own
/// one-block remedy sentence) rather than flagging every tier in the design,
/// which is the bug this exists to fix (a pavilion-only anchor failure used to
/// paint the crown's tiers "?" too).
fn missing_anchor_tier_blocks(missing: &MissingAnchor, design: &Design) -> BTreeMap<usize, Block> {
    missing
        .block_details(design)
        .into_iter()
        .flat_map(|(block, indices)| indices.into_iter().map(move |index| (index, block)))
        .collect()
}

/// One tier's display label for a validation-banner mention -- `"tier 5
/// (Girdle)"` when named, else `"tier 5"`. 1-based to match the tier table's
/// own `#` column, which is what a cutter actually reads off screen.
fn tier_label(design: &Design, tier_index: usize) -> String {
    design.tiers.get(tier_index).map_or_else(
        || format!("tier {}", tier_index + 1),
        |tier| {
            if tier.name.is_empty() {
                format!("tier {}", tier_index + 1)
            } else {
                format!("tier {} ({})", tier_index + 1, tier.name)
            }
        },
    )
}

/// The tier(s) [`Design::facet_meets`] actually resolved tier `index`'s meet
/// constraint against, as a display string (e.g. `"meets tier 3 (C1)"`,
/// `"meets tier 2 (C1), tier 4 (C2)"`), or `""` when it resolves to nothing
/// (a `ScaleReference`/`MeetExisting` tier, or a `MeetNamed` tier every one of
/// whose names is unresolved). Uses the same solver-grade [`MeetNameResolver`]
/// the solver itself runs, unlike guessing from the typed `constraint_text`
/// alone, so this shows the meet partners `MeetNameResolver` actually resolved.
/// Never needs a solve (`facet_meets` only resolves
/// names against `meet_tier_inputs`), so this is populated identically by
/// [`tier_items`] and [`tier_items_stale`].
fn meet_partners_text(design: &Design, index: usize) -> String {
    let Ok(targets) = design.facet_meets(index) else {
        return String::new();
    };
    if targets.is_empty() {
        return String::new();
    }
    format!(
        "meets {}",
        targets
            .into_iter()
            .map(|target| tier_label(design, target))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// [`indicatrix::geometry::stone_metrics::SolidStatus::Unbounded`]'s escaping
/// plane indices, named by the tier that contributed each one
/// ([`Design::tier_for_plane_index`]) instead of shown as raw indices into the
/// combined plane arrangement. Falls back to `"plane <n>"` for a preform plane
/// or an index [`Design::tier_for_plane_index`] can't place -- both mean "not
/// a schedule-tier facet," not a bug worth panicking over here.
fn escaping_tier_text(design: &Design, solved: &[SolvedTier], escaping: &[usize]) -> String {
    escaping
        .iter()
        .map(|&plane_index| {
            design
                .tier_for_plane_index(solved, plane_index)
                .map_or_else(
                    || format!("plane {plane_index}"),
                    |tier_index| tier_label(design, tier_index),
                )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// [`indicatrix::geometry::stone_metrics::SolidStatus::Degenerate`]'s suspect
/// tiers ([`indicatrix_cut_core::degenerate_suspects`]) as a trailing clause,
/// e.g. `" -- check tier 5 (Girdle), tier 8"` -- `""` when nothing is
/// suspect (every tier's mast came from a real anchor or real vertex-derived
/// structure, so a degenerate result has no single tier more likely at fault
/// than another).
fn degenerate_suspects_note(design: &Design, solved: &[SolvedTier]) -> String {
    let suspects = degenerate_suspects(solved);
    if suspects.is_empty() {
        return String::new();
    }
    let names: Vec<String> = suspects
        .into_iter()
        .map(|index| tier_label(design, index))
        .collect();
    format!(" -- check {}", names.join(", "))
}

/// The mast/strategy-derived half of one tier row -- [`build_tier_row`]'s only
/// per-tier-varying input beyond the tier itself, bundled into one struct so that
/// function stays under clippy's argument-count lint, instead of
/// [`tier_items`]/[`tier_items_stale`]/[`tier_items_from_solved`] each building
/// the whole [`EditorTierItem`] literal inline, three times over.
struct TierSolveInfo {
    mast: String,
    /// The mast in millimetres, empty when nothing anchors a real scale -- see
    /// [`EditorTierItem::mast_mm`].
    mast_mm: String,
    /// The same mast unrounded -- see [`EditorTierItem::mast_full`].
    mast_full: String,
    strategy: String,
    strategy_is_uncertain: bool,
    strategy_detail: String,
    needs_anchor: bool,
}

/// [`build_tier_row`]'s per-DESIGN (not per-tier) context, bundled into one
/// struct purely to keep that function under clippy's argument-count lint --
/// the same reasoning [`TierSolveInfo`] (per-tier) is already split out for.
/// Computed once by each of [`tier_items`]/[`tier_items_stale`]/
/// [`tier_items_stale_with_last_solved`]/[`tier_items_from_solved`] before
/// their own per-tier `.map(...)`, not per row.
struct RowContext<'a> {
    tier_blocks: &'a [Block],
    warnings: &'a BTreeMap<usize, String>,
    /// See [`tier_margin_and_risk`]'s own doc comment.
    pavilion_partner_deg: Option<f64>,
}

/// Builds one [`EditorTierItem`] row from `tier`'s own fields plus its
/// already-resolved [`TierSolveInfo`] -- the part [`tier_items`],
/// [`tier_items_stale`] and [`tier_items_from_solved`] all do identically once
/// they've each worked out mast/strategy/detail their own way (a real solve, no
/// solve at all, or an externally-supplied `solved` list, respectively).
/// `multi_selected` is always `false` here: none of the three callers above has
/// access to `EditorState::multi_selected` (deliberately not a parameter, even
/// indirectly -- see [`apply_multi_selection`]'s own doc comment for why that's
/// a post-pass instead); every real push site calls it on the built `Vec`
/// immediately afterward.
fn build_tier_row(
    design: &Design,
    index: usize,
    tier: &ConstraintTier,
    n_d: f64,
    info: TierSolveInfo,
    row_context: &RowContext<'_>,
) -> EditorTierItem {
    let tier_blocks = row_context.tier_blocks;
    let warnings = row_context.warnings;
    let pavilion_partner_deg = row_context.pavilion_partner_deg;
    let (constraint_kind, constraint_text) =
        constraint_kind_and_text(&tier.constraint, design.tier_target(index));
    let units = indicatrix_cut_core::orbit_units(&tier.indices, &design.meta);
    let (orbit_status, orbit_incomplete) = orbit_status_text(&units);
    let (margin_text, risk_level) = tier_margin_and_risk(tier.angle_deg, n_d, pavilion_partner_deg);
    EditorTierItem {
        index: index as i32,
        angle_deg: format_angle_cell(tier.angle_deg).into(),
        name: tier.name.clone().into(),
        indices: tier
            .indices
            .iter()
            .copied()
            .map(format_index_value)
            .collect::<Vec<_>>()
            .join(", ")
            .into(),
        constraint_kind,
        constraint_text: constraint_text.into(),
        mast: info.mast.into(),
        mast_mm: info.mast_mm.into(),
        mast_full: info.mast_full.into(),
        strategy: info.strategy.into(),
        strategy_is_uncertain: info.strategy_is_uncertain,
        strategy_detail: info.strategy_detail.into(),
        needs_anchor: info.needs_anchor,
        imported_meet_text: imported_meet_text(tier.imported_meet.as_ref()).into(),
        orbit_status: orbit_status.into(),
        orbit_incomplete,
        is_detached: !tier.detached.is_empty(),
        block: block_label(tier_blocks[index]).into(),
        margin_text: margin_text.into(),
        risk_level,
        meet_partners_text: meet_partners_text(design, index).into(),
        warning_text: warnings.get(&index).cloned().unwrap_or_default().into(),
        multi_selected: false,
        // Patched in place afterward, once there is a whole row list to patch --
        // see `apply_proposed_angles`'s own doc comment.
        proposed_angle: String::new().into(),
    }
}

/// Builds the tier list's rows WITHOUT calling [`Design::solve`] at all -- every
/// mast/strategy cell reads `"-"`/`"not solved"` (flagged uncertain) regardless of
/// what the design actually is. Used by `refresh_editor_panel_stale` after every edit
/// that isn't the explicit "Solve" action -- see this group's `mod.rs` doc comment for
/// why: a real design can take multiple seconds to solve, and showing a PREVIOUS
/// solve's masts would be actively wrong the moment the edit changed tier count/order.
pub(super) fn tier_items_stale(design: &Design, n_d: f64) -> Vec<EditorTierItem> {
    let tier_blocks = classify_blocks(&design.meet_tier_inputs());
    // Never solved here (see this function's own doc comment), so only the two
    // mast-free manufacturability checks can run -- tagged "(pre-solve)" since
    // that is the whole design's state right now, not a completed pass.
    let warnings = warning_text_by_tier(&manufacturability_warnings_tagged(design, None));
    let (_, pavilion_partner_deg) = representative_crown_and_pavilion_angles_deg(design);
    let row_context = RowContext {
        tier_blocks: &tier_blocks,
        warnings: &warnings,
        pavilion_partner_deg,
    };
    design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let info = TierSolveInfo {
                mast: "-".to_string(),
                mast_mm: String::new(),
                mast_full: String::new(),
                strategy: "not solved".to_string(),
                strategy_is_uncertain: true,
                // Never solved here (see this function's own doc comment), so
                // there is no per-tier detail or missing-anchor block to report
                // yet -- both come back once the explicit "Solve" action calls
                // `tier_items`.
                strategy_detail: String::new(),
                needs_anchor: false,
            };
            build_tier_row(design, index, tier, n_d, info, &row_context)
        })
        .collect()
}

/// [`tier_items_stale`]'s counterpart for a caller that still has the LAST real
/// solve's mast list on hand: [`tier_items_stale`] blanks every mast/strategy/
/// warning to `"-"`/`"not solved"` until the next solve completes, even for an
/// edit like a rename that cannot possibly move a mast. On a slow design (auto-solve
/// off, or over its own measured budget -- see `should_schedule_auto_solve`) that
/// wipes 100+ mast readings the cutter may be comparing against a gauge, for no
/// reason: a rename never invalidates anyone's mast. This function instead keeps
/// the previous solve's readings for every tier the edit didn't touch.
///
/// Rows named in `dirty` (the edit's own affected tier indices -- e.g. the single
/// index a Save Tier/inline-angle/wheel-nudge edit touched, or every index for a
/// structural edit like Undo/Redo/remove/duplicate/detach/gear-remap that can move
/// masts application-wide) get [`tier_items_stale`]'s own `"-"`/`"not solved"`
/// treatment -- no PREVIOUS mast can be trusted for a tier that itself just changed.
/// Every other row keeps `last_solved`'s own mast/strategy/detail, with strategy
/// prefixed `"stale"` so it never reads as a fresh, just-completed solve.
///
/// `last_solved` (and therefore every row) falls back to [`tier_items_stale`]'s
/// blank treatment whenever `None`, OR whenever its length no longer matches
/// `design.tiers.len()` -- a tier add/remove/reorder invalidates every index in an
/// old solve's list, so trusting it positionally would show tier 5's old mast on
/// today's tier 6.
///
/// # Handoff
/// `state/mod.rs` only computes; the caller needs a cached `Vec<SolvedTier>` from
/// this design's last real solve (`auto_solve::solid_last_solved`'s shared handle
/// already holds exactly this, refreshed by every completed background solve and
/// every solid-preview replan) and the dirty tier index set `tier_actions.rs`
/// already computes per edit (see `tier_actions.rs:687-695`) -- both read from
/// `view.rs`'s `push_stale_content`/`refresh_editor_panel_stale` (not this
/// file), which would call this in place of [`tier_items_stale`].
#[must_use]
pub(super) fn tier_items_stale_with_last_solved(
    design: &Design,
    n_d: f64,
    last_solved: Option<&[SolvedTier]>,
    dirty: &BTreeSet<usize>,
) -> Vec<EditorTierItem> {
    let last_solved = last_solved.filter(|solved| solved.len() == design.tiers.len());
    let Some(last_solved) = last_solved else {
        return tier_items_stale(design, n_d);
    };
    let tier_blocks = classify_blocks(&design.meet_tier_inputs());
    // A cached solve is still real evidence for the mast-free checks even though
    // it may now be one edit old -- tagged "(pre-solve)" regardless, same as
    // `tier_items_stale`, since a fresh edit landed since it ran and nothing here
    // re-verifies it still holds.
    let warnings = warning_text_by_tier(&manufacturability_warnings_tagged(design, None));
    let mm_per_unit = design.yield_report(last_solved).mm_per_unit;
    let (_, pavilion_partner_deg) = representative_crown_and_pavilion_angles_deg(design);
    let row_context = RowContext {
        tier_blocks: &tier_blocks,
        warnings: &warnings,
        pavilion_partner_deg,
    };
    design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let info = if dirty.contains(&index) {
                TierSolveInfo {
                    mast: "-".to_string(),
                    mast_mm: String::new(),
                    mast_full: String::new(),
                    strategy: "not solved".to_string(),
                    strategy_is_uncertain: true,
                    strategy_detail: String::new(),
                    needs_anchor: false,
                }
            } else {
                let (label, _) = strategy_label(last_solved[index].strategy);
                TierSolveInfo {
                    mast: format!("{:.4}", last_solved[index].mast),
                    mast_mm: mm_per_unit.map_or_else(String::new, |mm| {
                        format!("{:.3} mm", last_solved[index].mast * mm)
                    }),
                    mast_full: format!("{}", last_solved[index].mast),
                    strategy: format!("stale ({label})"),
                    strategy_is_uncertain: true,
                    strategy_detail: last_solved[index].detail.clone(),
                    needs_anchor: false,
                }
            };
            build_tier_row(design, index, tier, n_d, info, &row_context)
        })
        .collect()
}

/// Patches `EditorTierItem::proposed_angle`
/// (`ui/types.slint`) onto each row one of `changes`' own
/// [`indicatrix_cut_core::AngleChange`]s targets -- called AFTER the row list is
/// already built, the same "patch the already-pushed row list in place" shape
/// [`apply_multi_selection`] already uses for the multi-select highlight,
/// rather than threading an [`OptimizeOutcome`] through [`tier_items`]/
/// [`tier_items_stale`]/[`tier_items_from_solved`]/
/// [`tier_items_stale_with_last_solved`]'s four independent call sites.
///
/// Leaves `proposed_angle` at its default (`""`) for every row `changes` does
/// not mention. Formatted with [`format_angle_cell`]'s own two-decimal
/// convention, so a ghost value reads exactly like the real `angle_deg` cell
/// beside it.
///
/// # Handoff
/// `ui/components/editor_tier_table.slint` still
/// needs the ANGLE column's own rendering of this field -- e.g. a small
/// "\u{2192} 41.20\u{b0}" ghost beside the real value.
pub(super) fn apply_proposed_angles(
    tiers: &mut [EditorTierItem],
    changes: &[indicatrix_cut_core::AngleChange],
) {
    for change in changes {
        if let Some(row) = tiers.get_mut(change.index) {
            row.proposed_angle = format_angle_cell(change.to_deg).into();
        }
    }
}

/// Converts `design`'s current tier list into the rows `EditorView`'s list renders,
/// including the solved mast and [`SolveStrategy`] label for each. `index` is the
/// tier's position in `design.tiers` -- round-tripped back by
/// `EditorView.save_tier`/`remove_tier`, a list index rather than a stable id.
///
/// Solves `design` exactly once up front. When [`Design::solve`] returns
/// [`indicatrix_cut_core::MissingAnchor`], only the tiers that error actually
/// names (via [`missing_anchor_tier_blocks`]) get the "no anchor yet" `"?"`
/// treatment; every other tier -- blocked only because some OTHER block has no
/// anchor, not because it lacks one itself -- reads `"-"`/`"blocked"` instead,
/// so a pavilion-only failure no longer paints the crown's tiers "?" too. See
/// [`status_text_and_is_problem`], which surfaces which block(s) are missing an
/// anchor in the validation banner.
///
/// Only called from `refresh_all` (New/Load/the explicit "Solve" action) -- see
/// [`tier_items_stale`] for the no-solve version every other edit callback uses.
pub(super) fn tier_items(design: &Design, n_d: f64) -> Vec<EditorTierItem> {
    let solved = design.solve();
    match &solved {
        Ok(rows) => tier_items_from_solved(design, rows, n_d),
        Err(err) => {
            // Only a real `MissingAnchor` can name individual tiers -- a
            // `TierTarget`/mismatch/plane-cap failure blocks the whole design at
            // once, so every tier falls to the generic "blocked" arm below with
            // that error's own status-strip sentence rather than a bogus "no
            // anchor yet" on tiers that were never the problem.
            let missing_tier_blocks = match err {
                DesignSolveError::MissingAnchor(missing) => {
                    missing_anchor_tier_blocks(missing, design)
                }
                DesignSolveError::Mismatch(_)
                | DesignSolveError::Solve(_)
                | DesignSolveError::Target(_) => BTreeMap::new(),
            };
            let blocked_detail = if matches!(err, DesignSolveError::MissingAnchor(_)) {
                "Another block in this design has no anchor -- see the validation banner above."
                    .to_string()
            } else {
                err.to_string()
            };
            let tier_blocks = classify_blocks(&design.meet_tier_inputs());
            // Tagged "(pre-solve)" -- see `manufacturability_warnings_tagged`'s
            // own doc comment; there is no real solve to show warnings from yet.
            let warnings = warning_text_by_tier(&manufacturability_warnings_tagged(design, None));
            let (_, pavilion_partner_deg) = representative_crown_and_pavilion_angles_deg(design);
            let row_context = RowContext {
                tier_blocks: &tier_blocks,
                warnings: &warnings,
                pavilion_partner_deg,
            };
            design
                .tiers
                .iter()
                .enumerate()
                .map(|(index, tier)| {
                    let info = missing_tier_blocks.get(&index).map_or_else(
                        || TierSolveInfo {
                            mast: "-".to_string(),
                            mast_mm: String::new(),
                            mast_full: String::new(),
                            strategy: "blocked".to_string(),
                            strategy_is_uncertain: true,
                            strategy_detail: blocked_detail.clone(),
                            needs_anchor: false,
                        },
                        |&block| TierSolveInfo {
                            mast: "?".to_string(),
                            mast_mm: String::new(),
                            mast_full: String::new(),
                            strategy: "no anchor yet".to_string(),
                            strategy_is_uncertain: true,
                            strategy_detail: MissingAnchor::block_sentence(block),
                            needs_anchor: true,
                        },
                    );
                    build_tier_row(design, index, tier, n_d, info, &row_context)
                })
                .collect()
        }
    }
}

/// [`tier_items`]'s counterpart for a caller that already has an up-to-date
/// `solved` mast list on hand -- builds every row's mast/strategy/detail straight
/// from it instead of calling [`Design::solve`] again. Exists so the
/// solid-preview worker's own replan solve (`solid_preview::live_update::
/// plan_preview`, run off the UI thread) can feed the SAME masts into the tier
/// table instead of a second, separately dispatched full solve recomputing them --
/// see `gui::editor::view::push_solved_preview`, this
/// function's one caller.
///
/// # Panics
///
/// `solved` must have one entry per tier `design` currently has, in the same
/// order -- the same alignment contract [`Design::to_asc_schedule_from_solved`]
/// documents; indexing out of that range panics.
pub(super) fn tier_items_from_solved(
    design: &Design,
    solved: &[SolvedTier],
    n_d: f64,
) -> Vec<EditorTierItem> {
    let tier_blocks = classify_blocks(&design.meet_tier_inputs());
    let warnings = warning_text_by_tier(&manufacturability_warnings_tagged(design, Some(solved)));
    // `None` whenever the design carries no girdle diameter, which is
    // the only thing that anchors model units to a real size. Computed once here
    // rather than per row -- `yield_report` measures the whole solid.
    let mm_per_unit = design.yield_report(solved).mm_per_unit;
    let (_, pavilion_partner_deg) = representative_crown_and_pavilion_angles_deg(design);
    let row_context = RowContext {
        tier_blocks: &tier_blocks,
        warnings: &warnings,
        pavilion_partner_deg,
    };
    design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let (label, uncertain) = strategy_label(solved[index].strategy);
            let info = TierSolveInfo {
                mast: format!("{:.4}", solved[index].mast),
                mast_mm: mm_per_unit.map_or_else(String::new, |mm| {
                    format!("{:.3} mm", solved[index].mast * mm)
                }),
                mast_full: format!("{}", solved[index].mast),
                strategy: label.to_string(),
                strategy_is_uncertain: uncertain,
                strategy_detail: solved[index].detail.clone(),
                needs_anchor: false,
            };
            build_tier_row(design, index, tier, n_d, info, &row_context)
        })
        .collect()
}

/// [`EditorTierItem::block`]'s one-word label for a [`Block`] -- shared by
/// [`tier_items`]/[`tier_items_stale`] so a table row's crown/pavilion/girdle side
/// (silently governed by the unsigned-zero inheritance rule, `tier.rs`'s own doc
/// comment) is never left to be guessed from the angle's sign alone.
const fn block_label(block: Block) -> &'static str {
    match block {
        Block::Crown => "Crown",
        Block::Pavilion => "Pavilion",
        Block::Girdle => "Girdle",
    }
}

/// [`indicatrix_cut_core::manufacturability::check_manufacturability_available`]'s
/// findings against `design`'s current state, as `(tier_index, display_text)`
/// pairs -- lets a caller attribute a warning to its row
/// (`ManufacturabilityWarning::tier_index`) instead of only a flattened
/// `String`. `solved` is an already-[`Design::solve`]'d
/// mast list when one is available; passing `None` still runs the two
/// mast-free checks (gear quantization, cut order) -- see
/// [`check_manufacturability_available`](indicatrix_cut_core::manufacturability::check_manufacturability_available)'s
/// own doc comment: a design that has never solved, or no
/// longer does, must not lose every finding, only the two that genuinely need
/// a mesh.
pub(super) fn manufacturability_warnings_by_tier(
    design: &Design,
    solved: Option<&[SolvedTier]>,
) -> Vec<(usize, String)> {
    indicatrix_cut_core::manufacturability::check_manufacturability_available(
        design,
        solved,
        indicatrix_cut_core::manufacturability::DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    )
    .iter()
    .map(|warning| (warning.tier_index(), warning.to_string()))
    .collect()
}

/// [`manufacturability_warnings_by_tier`], with each finding's text prefixed
/// `"(pre-solve) "` when `solved` is `None` -- every caller that shows these
/// findings without a completed solve backing them must say so: the two
/// mast-free checks are real and actionable before Solve ever
/// runs, but must never be mistaken for a full manufacturability pass once the
/// mesh checks are back in play too.
pub(super) fn manufacturability_warnings_tagged(
    design: &Design,
    solved: Option<&[SolvedTier]>,
) -> Vec<(usize, String)> {
    let pairs = manufacturability_warnings_by_tier(design, solved);
    if solved.is_some() {
        return pairs;
    }
    pairs
        .into_iter()
        .map(|(index, text)| (index, format!("(pre-solve) {text}")))
        .collect()
}

/// Groups [`manufacturability_warnings_by_tier`]/[`manufacturability_warnings_tagged`]'s
/// pairs by tier index, joining more than one finding for the same tier with
/// `"; "` -- what [`tier_items`]/[`tier_items_stale`] feed into each row's
/// [`EditorTierItem::warning_text`].
fn warning_text_by_tier(pairs: &[(usize, String)]) -> BTreeMap<usize, String> {
    let mut grouped: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (tier_index, text) in pairs {
        grouped.entry(*tier_index).or_default().push(text.clone());
    }
    grouped
        .into_iter()
        .map(|(index, texts)| (index, texts.join("; ")))
        .collect()
}

/// The material `ComboBox`'s preset names, in EXACT index order -- index 0 is
/// `"(none)"`; every following index is a [`indicatrix_cut_core::MaterialCatalogue`]
/// built-in name, in that catalogue's own order (which is
/// `GemMaterial::all_materials`'s order, unchanged by this function).
///
/// This lists every built-in species the renderer supports, not just a
/// hand-picked THIRTEEN-name subset (`"Diamond"`..`"Cubic Zirconia"`) of the
/// renderer's THIRTY-TWO built-ins -- every garnet, Aquamarine, Morganite,
/// Chrysoberyl (Yellow), Amethyst, Citrine, Peridot, YAG, GGG, Benitoite,
/// Andalusite, Opal, both glasses and Rutile can already be traced in Live
/// Render (`gui::startup_settings::built_in_material_option_names` already reads
/// `GemMaterial::all_materials()` directly), and can also be NAMED on a design
/// or offered by the New Design dialog. `MaterialCatalogue::build(&[])` -- no
/// custom materials, since this is the CAD side's built-in-only picker -- is
/// the one place that decides "every built-in species exists", so this list
/// cannot drift from the renderer's own.
///
/// `new_design_dialog.slint`'s New Design material `ComboBox`
/// binds its `model` to `EditorModel.new_material_options`,
/// pushed from this same function every refresh (`view.rs`) -- see that property's
/// own doc comment (`ui/models/editor.slint`).
pub(super) fn builtin_preset_names() -> Vec<String> {
    std::iter::once("(none)".to_string())
        .chain(indicatrix_cut_core::MaterialCatalogue::build(&[]).names())
        .collect()
}

/// [`builtin_preset_names`]'s index for `name` (`None` -> `0`, an unrecognized name
/// -> `0` as a safe fallback rather than an out-of-range `ComboBox` index).
pub(super) fn material_index_from_name(name: Option<&str>) -> i32 {
    let names = builtin_preset_names();
    name.and_then(|n| names.iter().position(|p| p == n))
        .map_or(0, |i| i as i32)
}

/// The inverse of [`material_index_from_name`]: the name at `index` in
/// [`builtin_preset_names`], or `None` for index `0` ("(none)") or an out-of-range
/// index (defensive only -- `EditorView`'s `ComboBox` can never produce one).
pub(super) fn material_name_from_index(index: i32) -> Option<String> {
    let names = builtin_preset_names();
    usize::try_from(index)
        .ok()
        .and_then(|i| names.get(i).cloned())
        .filter(|name| name != "(none)")
}

/// Parses the Yield form's three fields into
/// [`indicatrix_cut_core::Edit::SetGirdleDiameterMm`]/[`indicatrix_cut_core::Edit::SetMaterial`]'s
/// payloads. Pure and unit tested directly. An empty `girdle_diameter_mm`/
/// `specific_gravity_override` field parses to `None` (the "unset, use the preset's
/// own figure" state), never an error: blank is a valid, deliberate choice here,
/// unlike the preform's three fields, which always name a real dimension.
///
/// The returned [`MaterialSelection`] is `current.with_specific_gravity_override(..)`
/// -- `name`/`refractive_index_override` always carry through from `current`
/// unchanged. Deriving `name` from `material_index` against
/// [`builtin_preset_names`] (built-ins only) instead would silently rename a
/// custom catalogue material to "(none)" (since `material_index_from_name` has
/// no custom entry to map it to) and would drop the RI override on every
/// "Apply Yield Inputs" click, even one that only touched the girdle diameter --
/// the Yield tab's own Material combo can only ever name a built-in preset or
/// "(none)", so it has no way to name a custom material correctly either.
/// `_material_index` is consequently unread: it stays a
/// parameter (prefixed `_`) only so this signature keeps matching the
/// existing `on_apply_yield_inputs` call site; the combo itself is
/// inert for naming purposes here (changing it and clicking Apply does not rename the
/// design's material) -- redesigning or removing that
/// control is a separate UI decision.
pub(super) fn parse_yield_form(
    girdle_diameter_mm: &str,
    _material_index: i32,
    specific_gravity_override: &str,
    current: &MaterialSelection,
) -> Result<(Option<f64>, MaterialSelection), String> {
    let girdle_diameter_mm = if girdle_diameter_mm.trim().is_empty() {
        None
    } else {
        let value: f64 = girdle_diameter_mm.trim().parse().map_err(|_| {
            format!(
                "Girdle diameter '{}' is not a number.",
                girdle_diameter_mm.trim()
            )
        })?;
        if !value.is_finite() || value <= 0.0 {
            return Err("Girdle diameter must be a positive, finite number.".to_string());
        }
        Some(value)
    };

    let specific_gravity_override = if specific_gravity_override.trim().is_empty() {
        None
    } else {
        let value: f64 = specific_gravity_override.trim().parse().map_err(|_| {
            format!(
                "Specific gravity '{}' is not a number.",
                specific_gravity_override.trim()
            )
        })?;
        if !value.is_finite() || value <= 0.0 {
            return Err("Specific gravity override must be a positive, finite number.".to_string());
        }
        Some(value)
    };

    Ok((
        girdle_diameter_mm,
        current.with_specific_gravity_override(specific_gravity_override),
    ))
}

/// `design`'s current yield/weight figures, formatted for `EditorView`'s read-only
/// display fields -- rides along with the ordinary Solve action (like
/// [`manufacturability_warning_lines`]) since `Design::yield_report` takes an
/// already-solved mast list.
///
/// Returns `(volumetric_yield_text, carat_weight_text, specific_gravity_used_text,
/// preform_fit_warning_text)` -- all four empty when the design does not currently
/// solve ([`indicatrix_cut_core::MissingAnchor`], already surfaced by the banner).
///
/// `custom_sg` is the catalogue's custom-material specific-gravity table --
/// see [`yield_report_texts_from_solved`]'s own doc comment for where a
/// caller sources it from.
pub(super) fn yield_report_texts(
    design: &Design,
    custom_sg: &[(String, f64)],
) -> (String, String, String, String) {
    let Ok(solved) = design.solve() else {
        return (String::new(), String::new(), String::new(), String::new());
    };
    yield_report_texts_from_solved(design, &solved, custom_sg)
}

/// [`yield_report_texts`]'s counterpart for a caller that already has an
/// up-to-date `solved` mast list on hand -- see [`tier_items_from_solved`]'s own
/// doc comment for why this exists. Never re-solves, and
/// never returns the all-empty tuple [`yield_report_texts`] falls back to on a
/// `MissingAnchor`: a caller holding a real `solved` slice already knows the
/// design solves.
///
/// Resolves `design.material`'s specific gravity through
/// [`EditorMaterialLookup`] instead of only [`Design::yield_report`]'s built-in
/// table, so a CUSTOM catalogue material's own recorded SG reaches the
/// carat-weight estimate too -- not just a built-in preset or a per-design
/// override. This module has no live `RenderContext` of its own (a pure
/// `Design`-only view-model helper), so `custom_sg` is threaded in by the caller
/// instead -- `view.rs`/`auto_solve.rs` each read it straight off their own
/// `RenderContext::custom_material_specific_gravity` (cloned before crossing onto
/// a background thread, same as `RenderContext::custom_materials` already is), so
/// this stays a single source of truth with no thread-local mirror to drift out
/// of sync.
pub(super) fn yield_report_texts_from_solved(
    design: &Design,
    solved: &[SolvedTier],
    custom_sg: &[(String, f64)],
) -> (String, String, String, String) {
    let catalogue = EditorMaterialLookup::new(&[]).with_specific_gravity(custom_sg);
    let report = design.yield_report_with(solved, &catalogue);

    let volumetric_yield_text = report
        .volumetric_yield
        .map(|y| format!("{:.2}%", y * 100.0))
        .unwrap_or_default();
    let carat_weight_text = report
        .carat_weight
        .map(|c| format!("{c:.4} ct (est.)"))
        .unwrap_or_default();
    // "(override)" whenever the figure came from the user's typed number rather than
    // the selected preset's table figure.
    let specific_gravity_used_text = report.specific_gravity_used.map_or_else(String::new, |sg| {
        if design.material.specific_gravity_override.is_some() {
            format!("{sg:.3} (override)")
        } else {
            format!("{sg:.3}")
        }
    });
    let preform_fit_warning_text = report
        .preform_fit
        .map(|fit| fit.to_string())
        .unwrap_or_default();

    (
        volumetric_yield_text,
        carat_weight_text,
        specific_gravity_used_text,
        preform_fit_warning_text,
    )
}

/// `design`'s proportion readouts for the Preform tab's "Proportions" section
/// -- table %, crown height, pavilion depth, total depth, and length-to-width,
/// the figures a cutter actually quotes. Built from
/// [`Design::stone_proportions`] and converted to millimetres via
/// [`Design::yield_report`]'s own scale factor whenever a trusted one exists
/// (a girdle diameter is set and the design measures); otherwise shown in the
/// design's own mast units with no unit suffix, rather than guessing a scale.
///
/// Returns `(table_percent_text, crown_height_text, pavilion_depth_text,
/// total_depth_text, length_to_width_text)`, every one of them `"-"` when the
/// design has no tiers at all, does not solve, isn't currently a closed solid,
/// or (the two depth fields specifically) has no vertical girdle plane with a
/// live facet to measure from -- see
/// [`indicatrix::geometry::stone_metrics::StoneProportions`]'s own doc comment
/// for why those two are `Option`. Rides along with the explicit "Solve"
/// action (like [`yield_report_texts`]), not every edit.
///
/// A tierless design still solves (an empty
/// mast list is a valid, closed, zero-plane solve -- `Design::solve` never
/// rejects it), and `stone_proportions` then measures the bare preform block:
/// table 100%, crown/pavilion 0%, `total_depth` the preform's own depth. Those
/// are honest numbers about the preform and fabricated ones about a stone that
/// does not exist yet, so the `tiers.is_empty()` guard below runs BEFORE the
/// solve -- including for `total_depth`, which (unlike the other four fields)
/// is not an `Option` on [`indicatrix::geometry::stone_metrics::StoneProportions`]
/// and so has no other way to read as "-". [`girdle_and_ratio_texts`] carries
/// the identical guard for the same reason; the `view` module's own
/// `proportions_texts_from_solved`/`girdle_and_ratio_texts_from_solved`
/// mirror it too, since a caller with an already-`solved` list came from a
/// design that solved -- which, per the above, a tierless one does.
pub(super) fn proportions_texts(design: &Design) -> (String, String, String, String, String) {
    let dash = || "-".to_string();
    if design.tiers.is_empty() {
        return (dash(), dash(), dash(), dash(), dash());
    }
    let Ok(solved) = design.solve() else {
        return (dash(), dash(), dash(), dash(), dash());
    };
    let Some(proportions) = design.stone_proportions(&solved) else {
        return (dash(), dash(), dash(), dash(), dash());
    };
    let mm_per_unit = design.yield_report(&solved).mm_per_unit;
    let proportions = mm_per_unit.map_or(proportions, |mm| proportions.to_mm(mm));
    let unit = if mm_per_unit.is_some() { " mm" } else { "" };
    let table_percent_text = proportions
        .table_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let crown_height_text = proportions
        .crown_height
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    let pavilion_depth_text = proportions
        .pavilion_depth
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    let total_depth_text = format!("{:.3}{unit}", proportions.total_depth);
    let length_to_width_text = proportions
        .length_to_width
        .map_or_else(dash, |v| format!("{v:.3}"));
    (
        table_percent_text,
        crown_height_text,
        pavilion_depth_text,
        total_depth_text,
        length_to_width_text,
    )
}

/// The three proportion readouts
/// [`proportions_texts`] does not expose -- girdle thickness (a figure with
/// no existing text home at all) and the printed `C/W%`/`P/W%` ratios every
/// faceting diagram actually prints, as opposed to the absolute crown/pavilion
/// depths [`proportions_texts`] already returns. `Design::stone_proportions` has
/// already computed all four (`StoneProportions::girdle_thickness`/`crown_to_width_percent`/
/// `pavilion_to_width_percent`/`girdle_to_width_percent`, see
/// [`indicatrix::geometry::stone_metrics::StoneProportions`]);
/// nothing under `gui::editor` reads any of them (grepped `state/mod.rs`/
/// `view.rs` for all four field names).
///
/// Mirrors [`proportions_texts`]'s own solve-then-measure shape, `"-"` fallback and
/// millimetre-vs-model-unit handling exactly (girdle thickness only -- the three
/// `_to_width_percent` fields are already scale-invariant percentages, exactly like
/// `table_percent`, so they never take the `unit` suffix), so the two families can
/// never disagree about which unit a given figure is shown in.
///
/// Returns `(girdle_thickness_text, crown_to_width_percent_text,
/// pavilion_to_width_percent_text, girdle_to_width_percent_text)`, every one of
/// them `"-"` under the exact same conditions [`proportions_texts`] already
/// documents (does not solve, isn't currently a closed solid, or -- all four here
/// specifically -- no vertical girdle plane with a live facet to measure the
/// girdle band from at all).
///
/// # Handoff
/// `state/mod.rs` only computes; `view.rs` owns
/// `refresh_editor_panel`'s call to [`proportions_texts`]/`proportions_texts_from_solved`
/// (view.rs:191-192) and would need a matching call to this function (and a small
/// local `_from_solved` mirror of it, exactly like `view.rs`'s own doc comment at
/// :58-64 already does for `proportions_texts_from_solved`) to reach the UI; the
/// four new strings would need a `EditorModel` property each (`ui/models/editor.slint`)
/// and a display row in `editor_inspector.slint`'s
/// "Proportions" section -- there is no girdle-thickness readout yet, and
/// crown/pavilion depth are still shown as absolute values, not the printed
/// C/W%/P/W% ratios.
#[must_use]
pub(super) fn girdle_and_ratio_texts(design: &Design) -> (String, String, String, String) {
    let dash = || "-".to_string();
    // A tierless design still solves, and `stone_proportions` then measures the bare
    // preform block: girdle 50% of a cube, crown and pavilion 0%. Those are honest
    // numbers about the preform and fabricated ones about the stone, so they must
    // never reach a proportions readout. The same
    // `tiers.is_empty()` guard protects `proportions_texts` (including its
    // `total_depth`) and both
    // `_from_solved` mirrors in `view.rs`, so every reader of these figures agrees.
    if design.tiers.is_empty() {
        return (dash(), dash(), dash(), dash());
    }
    let Ok(solved) = design.solve() else {
        return (dash(), dash(), dash(), dash());
    };
    let Some(proportions) = design.stone_proportions(&solved) else {
        return (dash(), dash(), dash(), dash());
    };
    let mm_per_unit = design.yield_report(&solved).mm_per_unit;
    let proportions = mm_per_unit.map_or(proportions, |mm| proportions.to_mm(mm));
    let unit = if mm_per_unit.is_some() { " mm" } else { "" };
    let girdle_thickness_text = proportions
        .girdle_thickness
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    let crown_to_width_percent_text = proportions
        .crown_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let pavilion_to_width_percent_text = proportions
        .pavilion_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let girdle_to_width_percent_text = proportions
        .girdle_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    (
        girdle_thickness_text,
        crown_to_width_percent_text,
        pavilion_to_width_percent_text,
        girdle_to_width_percent_text,
    )
}

/// One proportion metric's verdict against
/// [`indicatrix_cut_core::proportions_windows`]'s reference table -- `level`
/// is `0` ([`Verdict::Within`]), `1` ([`Verdict::Near`]), `2`
/// ([`Verdict::Outside`]), or `-1` ("nothing to judge yet": the design does
/// not currently solve/close, or this particular metric has no value to
/// judge -- e.g. no crown tier at all). `reason` is the matched window's own
/// one-line explanation, `""` at level `-1`.
pub(super) struct ProportionVerdict {
    pub(super) level: i32,
    pub(super) reason: String,
}

/// The five [`ProportionVerdict`]s the Preform tab's "Proportion guidance"
/// section shows, one per metric
/// [`indicatrix_cut_core::proportions_windows::Metric`] lists.
pub(super) struct ProportionVerdicts {
    pub(super) table_pct: ProportionVerdict,
    pub(super) crown_angle: ProportionVerdict,
    pub(super) pavilion_angle: ProportionVerdict,
    pub(super) total_depth_pct: ProportionVerdict,
    pub(super) girdle_pct: ProportionVerdict,
}

/// The "nothing to judge yet" verdict -- see [`ProportionVerdict`]'s own doc
/// comment for what level `-1` means.
const fn verdict_none() -> ProportionVerdict {
    ProportionVerdict {
        level: -1,
        reason: String::new(),
    }
}

/// Looks `value` up against `shape`/`material`/`metric`'s reference window
/// (via [`indicatrix_cut_core::proportions_windows::verdict_for`]) and turns
/// the result into a [`ProportionVerdict`] -- [`verdict_none`] when `value` is
/// `None` (there was nothing to measure) or the lookup itself found no window
/// at all (never happens for a real metric today -- see that function's own
/// doc comment on its Round/Mid fallback).
fn verdict_from(
    value: Option<f64>,
    shape: indicatrix_cut_core::ShapeClass,
    material: indicatrix_cut_core::MaterialClass,
    metric: indicatrix_cut_core::ProportionMetric,
) -> ProportionVerdict {
    let Some(v) = value else {
        return verdict_none();
    };
    let Some((verdict, window)) =
        indicatrix_cut_core::proportions_windows::verdict_for(shape, material, metric, v)
    else {
        return verdict_none();
    };
    let level = match verdict {
        indicatrix_cut_core::Verdict::Within => 0,
        indicatrix_cut_core::Verdict::Near => 1,
        indicatrix_cut_core::Verdict::Outside => 2,
    };
    ProportionVerdict {
        level,
        reason: window.reason.to_string(),
    }
}

/// This design's own [`indicatrix_cut_core::ShapeClass`], for the proportion
/// verdicts above. Only [`indicatrix_cut_core::ShapeClass::Round`] is ever
/// returned (a symmetry order of 6 or more with mirroring on -- the round
/// brilliant family, the one shape this app ships verified, closing templates
/// for, see [`indicatrix_cut_core::templates`]'s own doc comment); every other
/// schedule reads as [`indicatrix_cut_core::ShapeClass::Other`], the honest
/// default this crate has no real cushion/oval/step/trillion classifier for
/// (see `proportions_windows`'s own top doc comment -- `Other` still judges
/// against the same generic lapidary window `Round`'s own Mid/Low band uses).
const fn shape_class_for(
    meta: &indicatrix_cut_core::ScheduleMeta,
) -> indicatrix_cut_core::ShapeClass {
    if meta.symmetry_order >= 6 && meta.mirror {
        indicatrix_cut_core::ShapeClass::Round
    } else {
        indicatrix_cut_core::ShapeClass::Other
    }
}

/// The Preform tab's five proportion-verdict chips, judged against `design`'s already-solved
/// `proportions` -- the SAME [`indicatrix::geometry::stone_metrics::
/// StoneProportions`] the plain-number readouts above already read, so the
/// chip and the number next to it can never disagree about the underlying
/// measurement. `n_d` is the design's effective refractive index (the same
/// value [`view::refresh_design_settings`](super::super::view::refresh_design_settings)
/// pushes as `effective_ri_text`).
///
/// Total depth is judged as the SUM of `crown_to_width_percent` +
/// `pavilion_to_width_percent` + `girdle_to_width_percent` -- `StoneProportions`
/// has no standalone "total depth as a percentage of width" field of its own
/// (only the absolute `total_depth`, in model units/mm), and total depth is,
/// by construction, crown height plus pavilion depth plus girdle thickness, so
/// this sum is the honest percentage-of-width equivalent, not an approximation
/// invented for this function.
#[must_use]
pub(super) fn proportion_verdicts(
    design: &Design,
    proportions: &indicatrix::geometry::stone_metrics::StoneProportions,
    n_d: f64,
) -> ProportionVerdicts {
    let shape = shape_class_for(&design.meta);
    let material = indicatrix_cut_core::MaterialClass::from_ri(n_d);
    let (crown_angle_deg, pavilion_angle_deg) =
        representative_crown_and_pavilion_angles_deg(design);
    let total_depth_pct = match (
        proportions.crown_to_width_percent,
        proportions.pavilion_to_width_percent,
        proportions.girdle_to_width_percent,
    ) {
        (Some(c), Some(p), Some(g)) => Some(c + p + g),
        _ => None,
    };
    ProportionVerdicts {
        table_pct: verdict_from(
            proportions.table_percent,
            shape,
            material,
            indicatrix_cut_core::ProportionMetric::TablePercent,
        ),
        crown_angle: verdict_from(
            crown_angle_deg,
            shape,
            material,
            indicatrix_cut_core::ProportionMetric::CrownAngle,
        ),
        pavilion_angle: verdict_from(
            pavilion_angle_deg,
            shape,
            material,
            indicatrix_cut_core::ProportionMetric::PavilionAngle,
        ),
        total_depth_pct: verdict_from(
            total_depth_pct,
            shape,
            material,
            indicatrix_cut_core::ProportionMetric::TotalDepthPercent,
        ),
        girdle_pct: verdict_from(
            proportions.girdle_to_width_percent,
            shape,
            material,
            indicatrix_cut_core::ProportionMetric::GirdleThicknessPercent,
        ),
    }
}

/// The Preform tab's Half-Width/Depth fields, converted to
/// millimetres via the same [`Design::yield_report`] scale factor
/// [`proportions_texts`] uses -- those two fields are typed and stored in the
/// design's own mast-unit scale (girdle half-width = 1), which reads as
/// ambiguous next to the Yield section's "Girdle Diameter (mm)" a few fields
/// down. Returned ALONGSIDE the model-unit value, never in place of it (the
/// fields stay editable in model units -- `PreformSpec` itself has no mm
/// concept); each `""` when the design does not currently solve, or no girdle
/// diameter is set to anchor `mm_per_unit` at all.
#[must_use]
pub(super) fn preform_mm_texts(design: &Design) -> (String, String) {
    let Ok(solved) = design.solve() else {
        return (String::new(), String::new());
    };
    let Some(mm_per_unit) = design.yield_report(&solved).mm_per_unit else {
        return (String::new(), String::new());
    };
    let preform = &design.preform;
    (
        format!("\u{2248} {:.3} mm", preform.half_width * mm_per_unit),
        format!("\u{2248} {:.3} mm", preform.depth * mm_per_unit),
    )
}

/// `EditorModel.preform_y_offset_mm`'s seed
/// value -- `design.preform_y_offset` (model/mast units) converted to real
/// millimetres via `mm_per_unit`, the SAME anchor [`preform_mm_texts`] converts
/// Half-Width/Depth with. Unlike that function, this is pure (no internal
/// `Design::solve`): the caller already has `mm_per_unit` on hand from a
/// SOLVED mast list, or passes `None` when it deliberately never solves (`view::
/// push_stale_content`) -- `""` in that case, the same "cleared, not left
/// showing a superseded value" treatment `preform_mm_texts`' own two fields get
/// there. Formatted as a bare decimal (not `preform_mm_texts`' "\u{2248} ... mm"
/// style) because, unlike those two read-only displays, this field is the
/// editable value `apply_preform_y_offset`'s `Edit::SetPreformYOffset` round-
/// trips through -- an approximation glyph or unit suffix would not re-parse.
#[must_use]
pub(super) fn preform_y_offset_mm_text(preform_y_offset: f64, mm_per_unit: Option<f64>) -> String {
    mm_per_unit.map_or_else(String::new, |mm_per_unit| {
        format!("{:.2}", preform_y_offset * mm_per_unit)
    })
}

/// A short label naming the design currently under edit -- the paired `.asc`'s
/// bare file name when this design was loaded from (or saved to) one, else
/// `"Untitled design"`. This is the editor-side half,
/// shown in the status strip; the viewport/render-side half would need a
/// separate `RenderContext` change outside this file.
pub(super) fn design_label_text(asc_filename: Option<&str>) -> String {
    asc_filename.map_or_else(|| "Untitled design".to_string(), str::to_string)
}

/// `design`'s current cut order as the Edit tab's own schedule rows -- angle,
/// facet name, index positions and notes, in cut order -- so the cutter can
/// see the design actually being edited rather than only ever the catalogue's
/// original schedule. Built from
/// [`Design::try_to_asc_schedule_from_solved`], the same conversion "Export Edited
/// .asc" already uses, so this can never disagree with what an export writes.
///
/// Uses `try_to_asc_schedule_from_solved`
/// (not the panicking `to_asc_schedule_from_solved`) -- every real caller already
/// passes a `solved` it just derived from THIS SAME `design`, so a
/// [`SolveMismatch`] should not be reachable in practice, but this is a `pub(super)`
/// helper with several callers across this module, none of which should be able to
/// crash the editor over a design/solve pairing that slipped out of sync. Logs and
/// returns an empty schedule rather than panicking; every caller already treats an
/// empty `cutting_rows`/schedule as the ordinary "nothing to show yet" state.
pub(super) fn cutting_schedule_rows(design: &Design, solved: &[SolvedTier]) -> Vec<AngleItem> {
    // `AngleItem::side` comes from the solver's own block classification, not from
    // the sign of the angle: that is the same source the tier table's own block
    // column uses, and it distinguishes a girdle facet (neither side) from a crown
    // one, which a sign test cannot.
    let tier_blocks = classify_blocks(&design.meet_tier_inputs());
    let schedule = match design.try_to_asc_schedule_from_solved(solved) {
        Ok(schedule) => schedule,
        Err(mismatch) => {
            tracing::warn!(%mismatch, "cutting_schedule_rows: solved masts do not match this design's tier count");
            return Vec::new();
        }
    };
    schedule
        .tiers
        .into_iter()
        .enumerate()
        .map(|(order_idx, tier)| AngleItem {
            order_idx: order_idx as i32,
            side: match tier_blocks.get(order_idx) {
                Some(Block::Crown) => 1,
                Some(Block::Pavilion) => -1,
                _ => 0,
            },
            facet: if tier.name.is_empty() {
                format!("#{}", order_idx + 1)
            } else {
                tier.name
            }
            .into(),
            angle: format_angle_cell(tier.angle_deg).into(),
            index_val: tier
                .indices
                .into_iter()
                .map(format_index_value)
                .collect::<Vec<_>>()
                .join(", ")
                .into(),
            notes: tier.notes.into(),
        })
        .collect()
}

/// GemCad/GemCutStudio-style external proportion ratios (`L/W`, `H/W`, and
/// `C/W`/`P/W` where a live girdle facet exists) computed from `m`, so the
/// validation banner reports figures a cutter can compare against
/// GemCad/GCS's own recalculation output, not just a bare cubic-model-unit
/// volume that is meaningless next to what GemCad/GCS show.
/// The ratios are dimensionless, so no unit suffix is needed regardless of
/// whether the design has a real millimetre scale (unlike [`proportions_texts`],
/// which is -- these two intentionally never share a formatter, since the
/// banner's ratios and the Preform tab's absolute lengths answer different
/// questions). `""` when the solid measures zero width (never happens for a
/// real `Closed` solid, but avoids a division by zero).
///
/// `mm_per_unit` -- [`Design::yield_report`]'s own scale factor, `Some` only
/// when a girdle diameter is set and the design measures -- appends the finished
/// stone's absolute width x total-depth in millimetres alongside the ratios
/// above, the other half of what GemCad/GCS report after every recalculation
/// (the ratios alone still leave "how big is it really" to the Yield section).
/// `None` (no girdle diameter set yet) omits this clause entirely rather than
/// showing a guessed or model-unit figure next to genuine millimetres.
fn external_proportions_note(m: &SolidMetrics, mm_per_unit: Option<f64>) -> String {
    if m.width_axis <= 0.0 {
        return String::new();
    }
    let mut parts = vec![
        format!("L/W {:.3}", m.length_axis / m.width_axis),
        format!("H/W {:.3}", m.total_height / m.width_axis),
    ];
    if let Some(c) = m.crown_height {
        parts.push(format!("C/W {:.3}", c / m.width_axis));
    }
    if let Some(p) = m.pavilion_depth {
        parts.push(format!("P/W {:.3}", p / m.width_axis));
    }
    if let Some(mm) = mm_per_unit {
        parts.push(format!(
            "{:.2} x {:.2} mm",
            m.width_axis * mm,
            m.total_height * mm
        ));
    }
    format!(", {}", parts.join(", "))
}

/// [`Design::status`], rendered as the Edit tab's validation banner text plus whether
/// it should be styled as a problem (red) or all-clear (green). Every branch reads as
/// one clean sentence (or, for a multi-block [`MissingAnchor`], one full sentence per
/// block) -- no stray leading/trailing punctuation left over from wrapping a nested
/// message in another sentence.
///
/// `Unbounded`/`Degenerate` both name the actual tier(s) responsible
/// ([`escaping_tier_text`]/[`degenerate_suspects_note`]) rather than raw plane indices
/// or nothing at all -- see those functions' own doc comments. `Unbounded` is
/// "essentially unreachable" through `Design` in practice (the preform always caps
/// every direction), not provably impossible, so it still gets a real message rather
/// than being treated as dead code. Either can ALSO be [`auto_solve::too_many_planes_message`]'s
/// plane-cap sentence instead -- see that function's own doc comment.
pub(super) fn status_text_and_is_problem(design: &Design) -> (String, bool) {
    match design.status() {
        Ok(SolidStatus::Closed(_)) => {
            // `design.measure()` re-solves and re-meshes independently of `status()`
            // above (no caching), so it can in principle disagree about closure;
            // `.flatten()` treats that theoretical `Ok(None)` the same as an error --
            // either way there's no volume figure to show.
            let volume_note = design
                .measure()
                .ok()
                .flatten()
                .map(|m| {
                    // A second, independent `solve()` (same "no caching" reasoning
                    // as `measure()`'s own doc comment above) purely to read
                    // `yield_report`'s scale factor -- `status_text_and_is_problem_
                    // from_solved` below avoids this because it already has one.
                    let mm_per_unit = design
                        .solve()
                        .ok()
                        .and_then(|solved| design.yield_report(&solved).mm_per_unit);
                    format!(
                        " -- volume {:.4} (model units^3){}",
                        m.volume,
                        external_proportions_note(&m, mm_per_unit)
                    )
                })
                .unwrap_or_default();
            (format!("Closed solid{volume_note}."), false)
        }
        Ok(SolidStatus::Degenerate {
            vertex_count,
            volume,
        }) => {
            // Same "(model units^3)" suffix as the `Closed`
            // arm above, so the unit is never ambiguous the way a bare
            // "volume 0.1234" with no unit would be.
            let volume_text = volume.map_or_else(
                || "non-finite".to_string(),
                |v| format!("{v:.4} (model units^3)"),
            );
            // `status()` only reaches `Degenerate` after a real `solve()` (it builds
            // the mesh from `Self::planes`, which requires one), so this re-solve
            // should always succeed -- `unwrap_or_default` degrades to no suspects
            // clause rather than panicking on that assumption if it's ever wrong.
            // `.ok()` shared below with the cheap plane-cap pre-filter, so the
            // common (not-over-cap) case pays for exactly this one solve, never two.
            let solved = design.solve().ok();
            if solved
                .as_deref()
                .is_some_and(auto_solve::likely_hit_plane_cap)
                && let Some(message) = auto_solve::too_many_planes_message(design)
            {
                return (message, true);
            }
            let suspects_note = solved.as_ref().map_or(String::new(), |solved| {
                degenerate_suspects_note(design, solved)
            });
            (
                format!(
                    "Degenerate: only {vertex_count} distinct vertex(es), volume \
                     {volume_text}{suspects_note}."
                ),
                true,
            )
        }
        Ok(SolidStatus::Unbounded { escaping }) => {
            // Same re-solve reasoning as the `Degenerate` arm above.
            let solved = design.solve().ok();
            if solved
                .as_deref()
                .is_some_and(auto_solve::likely_hit_plane_cap)
                && let Some(message) = auto_solve::too_many_planes_message(design)
            {
                return (message, true);
            }
            let tier_text = solved.as_ref().map_or_else(
                || format!("plane(s) {escaping:?}"),
                |solved| escaping_tier_text(design, solved, &escaping),
            );
            (
                format!("Unbounded: {tier_text} never close the solid."),
                true,
            )
        }
        // `MissingAnchor`'s own `Display` is already the full actionable remedy --
        // one complete sentence per named block (e.g. "Pavilion has no anchor: add a
        // tier with an exact scale value.") -- so it is shown verbatim rather than
        // wrapped in another sentence around it.
        Err(missing) => (missing.to_string(), true),
    }
}

/// [`status_text_and_is_problem`]'s counterpart for a caller that already has an
/// up-to-date `solved` mast list on hand -- builds the mesh from
/// [`Design::planes_from_solved`] instead of re-solving via
/// [`Design::status`]/[`Design::measure`]. There is no
/// `MissingAnchor` arm here: a caller holding a real `solved` slice already
/// knows the design solves.
pub(super) fn status_text_and_is_problem_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> (String, bool) {
    let planes = design.planes_from_solved(solved);
    match build_solid_mesh(&planes) {
        SolidStatus::Closed(_) => {
            // Same "independent re-mesh, not cached" reasoning
            // `status_text_and_is_problem`'s own `Closed` arm documents. Unlike
            // that arm, `solved` is already on hand here, so no extra `solve()`
            // is needed to read `yield_report`'s scale factor.
            let mm_per_unit = design.yield_report(solved).mm_per_unit;
            let volume_note = measure_solid(&planes)
                .map(|m| {
                    format!(
                        " -- volume {:.4} (model units^3){}",
                        m.volume,
                        external_proportions_note(&m, mm_per_unit)
                    )
                })
                .unwrap_or_default();
            (format!("Closed solid{volume_note}."), false)
        }
        SolidStatus::Degenerate {
            vertex_count,
            volume,
        } => {
            // `solved` may itself be
            // `auto_solve::solve_cancellably`'s own over-`MAX_PLANES` fallback (an
            // all-`SolveStrategy::Failed` list built with no real solve at all, see
            // that function's own doc comment) -- this caller's own doc comment
            // ("a caller holding a real `solved` slice already knows the design
            // solves") does not hold for that one case. The cheap pre-filter
            // (`auto_solve::likely_hit_plane_cap`) reads `solved`'s own tells with no
            // new solve at all; only a hit re-verifies via `too_many_planes_message`
            // (a real `solve_with`) for the authoritative numbers.
            if auto_solve::likely_hit_plane_cap(solved)
                && let Some(message) = auto_solve::too_many_planes_message(design)
            {
                return (message, true);
            }
            // See `status_text_and_is_problem`'s matching arm.
            let volume_text = volume.map_or_else(
                || "non-finite".to_string(),
                |v| format!("{v:.4} (model units^3)"),
            );
            let suspects_note = degenerate_suspects_note(design, solved);
            (
                format!(
                    "Degenerate: only {vertex_count} distinct vertex(es), volume \
                     {volume_text}{suspects_note}."
                ),
                true,
            )
        }
        SolidStatus::Unbounded { escaping } => {
            if auto_solve::likely_hit_plane_cap(solved)
                && let Some(message) = auto_solve::too_many_planes_message(design)
            {
                return (message, true);
            }
            let tier_text = escaping_tier_text(design, solved, &escaping);
            (
                format!("Unbounded: {tier_text} never close the solid."),
                true,
            )
        }
    }
}

/// Converts `design`'s current plane arrangement into the `GpuFacetPlane`s the render
/// thread's viewport already knows how to draw. See this group's `mod.rs` doc comment
/// ("Feeding the viewport") for the sign-flip convention this inverts.
///
/// `design.planes()` returns `Result` ([`indicatrix_cut_core::MissingAnchor`] when
/// some block has no scale-reference tier yet). On `Err` this returns an empty plane
/// set rather than fabricating geometry: the viewport going blank is truthful (no
/// design to draw) rather than showing stale or invented facets.
pub(super) fn design_to_gpu_planes(design: &Design) -> Vec<GpuFacetPlane> {
    design
        .planes()
        .unwrap_or_default()
        .into_iter()
        .map(|(normal, offset)| GpuFacetPlane::new(normal.as_vec3(), -offset as f32))
        .collect()
}

/// The tier search/filter box's substring test -- Slint's
/// `string` has no `contains`/index-of operation, so this runs in Rust and is
/// exposed to `editor_tier_table.slint` via the `pure` `EditorModel.
/// tier_matches_filter` callback (see that property's own doc comment in
/// `ui/models/editor.slint` for the handoff this still needs). Case-insensitive; an
/// empty `filter` always matches (every call site also short-circuits on this in
/// Slint before ever calling here, but this stays correct standalone too).
#[must_use]
pub(super) fn tier_matches_filter(haystack: &str, filter: &str) -> bool {
    let filter = filter.trim();
    if filter.is_empty() {
        return true;
    }
    haystack.to_lowercase().contains(&filter.to_lowercase())
}

/// Explains what `Design::effective_refractive_index_with`'s current result (shown
/// as "Eff. RI" in the design settings panel) actually IS and where it came from,
/// so a cutter picking a custom catalogue material
/// can tell whether the critical angle/MARGIN/render/export are using
/// that material's real index or a stale fallback, because nothing on screen named
/// the source. Mirrors `Design::effective_refractive_index_with`'s own precedence
/// exactly (typed override, then a custom catalogue material by name, then a
/// built-in preset, then the design's legacy imported `I` line) so this text can
/// never disagree with the number it explains.
#[must_use]
pub(super) fn ri_source_text(material: &MaterialSelection, custom: &[GemMaterial]) -> String {
    if let Some(value) = material.refractive_index_override {
        return format!("typed override ({value:.4})");
    }
    if let Some(name) = material.name.as_deref() {
        if custom.iter().any(|gem| gem.name.eq_ignore_ascii_case(name)) {
            return format!("custom catalogue material '{name}'");
        }
        if indicatrix_cut_core::built_in_refractive_index(name).is_some() {
            return format!("built-in material '{name}'");
        }
        return format!(
            "'{name}' is not a recognized material -- falling back to this design's legacy imported value"
        );
    }
    "no material selected -- using this design's legacy imported value".to_string()
}

/// Appended to a custom material's own name when it collides
/// case-insensitively with a built-in already in the combo (`design_material_options`'s
/// list order guarantees the built-in's own plain entry always comes first, from
/// [`builtin_preset_names`]), so the combo shows a second, clearly-labeled entry
/// instead of silently dropping the custom material from the list entirely -- a
/// cutter selecting "Diamond" can tell that a custom "Diamond"
/// exists too, and that `EditorMaterialLookup`'s custom-over-built-in
/// precedence (`material_lookup.rs`) means it is the one actually rendered.
/// [`design_material_name_from_index`] strips this suffix back off before it ever
/// becomes a real [`MaterialSelection::name`] -- see that function's own doc comment.
const CUSTOM_BUILTIN_COLLISION_SUFFIX: &str = " (custom)";

/// The design settings panel's material combo, in the EXACT order it must list:
/// [`builtin_preset_names`] verbatim, then `custom`'s own names -- labeled with
/// [`CUSTOM_BUILTIN_COLLISION_SUFFIX`] for any that collides
/// case-insensitively with a built-in already listed, rather than skipped outright
/// -- then a final `"Custom RI…"` sentinel -- see [`design_material_name_from_index`]
/// for what selecting it means. Pure and cheap enough to call directly for a
/// one-off need; `view::refresh_design_settings`'s own per-refresh call instead goes
/// through [`EditorState::material_combo_options`], which caches this result and
/// only re-derives it when `custom`'s own name list has actually changed.
pub(super) fn design_material_options(custom: &[GemMaterial]) -> Vec<String> {
    let mut options: Vec<String> = builtin_preset_names();
    for material in custom {
        if options
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&material.name))
        {
            options.push(format!(
                "{}{CUSTOM_BUILTIN_COLLISION_SUFFIX}",
                material.name
            ));
        } else {
            options.push(material.name.clone());
        }
    }
    options.push("Custom RI\u{2026}".to_string());
    options
}

/// The inverse of [`design_material_name_from_index`]: `options`'s index for `name`
/// (case-insensitive, matched against the REAL name -- a labeled entry's own suffix is
/// stripped before comparing, so a stored `MaterialSelection::name` of "Diamond" still
/// finds a match even if the only remaining occurrence in `options` were the labeled
/// one), or `0` ("(none)") when `name` is absent or not found.
pub(super) fn design_material_index_from_name(name: Option<&str>, options: &[String]) -> i32 {
    name.and_then(|n| {
        options.iter().position(|o| {
            o.strip_suffix(CUSTOM_BUILTIN_COLLISION_SUFFIX)
                .unwrap_or(o)
                .eq_ignore_ascii_case(n)
        })
    })
    .map_or(0, |i| i as i32)
}

/// The name at `index` in `options`, or `None` for index `0` ("(none)"), the trailing
/// `"Custom RI…"` sentinel, or an out-of-range index. Selecting `"Custom RI…"`
/// clears `MaterialSelection::name` the same way "(none)" does -- it exists so a
/// species with no built-in or catalogue preset (e.g. garnet) can still be given a
/// real, typed refractive index without pretending to pick a preset that isn't used.
///
/// A [`CUSTOM_BUILTIN_COLLISION_SUFFIX`]-labeled entry
/// (`"Diamond (custom)"`) is stripped back to the real material name (`"Diamond"`)
/// here -- the label is a DISPLAY convenience only; the stored
/// [`MaterialSelection::name`] must stay the real name so it keeps resolving through
/// `super::material_lookup::EditorMaterialLookup`'s custom-over-built-in precedence
/// exactly like the plain built-in entry does (both name the same real material,
/// since the label exists only to show the collision, not to distinguish two
/// different selections).
pub(super) fn design_material_name_from_index(index: i32, options: &[String]) -> Option<String> {
    usize::try_from(index)
        .ok()
        .and_then(|i| options.get(i))
        .filter(|&name| name != "(none)" && name != "Custom RI\u{2026}")
        .map(|name| {
            name.strip_suffix(CUSTOM_BUILTIN_COLLISION_SUFFIX)
                .map_or_else(|| name.clone(), str::to_string)
        })
}

/// Parses the design settings panel's material combo index plus its RI override text
/// field into a new [`MaterialSelection`] -- reads `current` first and carries its
/// `specific_gravity_override` through unchanged (that stays the Yield panel's own
/// field), matching [`Edit::SetMaterial`]'s "wholesale, not per-field" contract. An
/// empty override field means `None` (use the resolved material's own `n_D`); a
/// non-blank field must parse as a finite refractive index strictly greater than 1.0
/// (no real gem material has RI <= 1.0).
pub(super) fn parse_design_material_form(
    combo_index: i32,
    ri_override_text: &str,
    options: &[String],
    current: &MaterialSelection,
) -> Result<MaterialSelection, String> {
    let refractive_index_override = if ri_override_text.trim().is_empty() {
        None
    } else {
        let value: f64 = ri_override_text.trim().parse().map_err(|_| {
            format!(
                "Refractive index '{}' is not a number.",
                ri_override_text.trim()
            )
        })?;
        if !value.is_finite() || value <= 1.0 {
            return Err(
                "Refractive index override must be a finite number greater than 1.0.".to_string(),
            );
        }
        Some(value)
    };
    Ok(MaterialSelection {
        name: design_material_name_from_index(combo_index, options),
        specific_gravity_override: current.specific_gravity_override,
        refractive_index_override,
    })
}

/// The gear combo's index for `gear_teeth` -- a position in [`GEAR_PRESETS`] when it
/// matches exactly, else `GEAR_PRESETS.len()` (the trailing "Custom" entry), matching
/// [`gear_choice_to_teeth`]'s inverse mapping.
pub(super) fn gear_index_from_teeth(gear_teeth: i32) -> i32 {
    GEAR_PRESETS
        .iter()
        .position(|&g| g == gear_teeth)
        .map_or(GEAR_PRESETS.len() as i32, |i| i as i32)
}

/// The inverse of [`gear_index_from_teeth`]: resolves the gear combo's selected index
/// (plus, only for the trailing "Custom" entry, the paired text field) to a real gear
/// tooth count. `preset_index` outside `0..=GEAR_PRESETS.len()` is defensive-only and
/// falls back to reading `custom_text`, same as an explicit "Custom" pick.
pub(super) fn gear_choice_to_teeth(preset_index: i32, custom_text: &str) -> Result<i32, String> {
    if let Ok(i) = usize::try_from(preset_index)
        && let Some(&teeth) = GEAR_PRESETS.get(i)
    {
        return Ok(teeth);
    }
    let teeth: i32 = custom_text.trim().parse().map_err(|_| {
        format!(
            "Gear tooth count '{}' is not a whole number.",
            custom_text.trim()
        )
    })?;
    if teeth <= 0 {
        return Err("Gear tooth count must be a positive whole number.".to_string());
    }
    Ok(teeth)
}

/// A real dry run of [`Edit::RemapIndices`] against a scratch clone of `design`
/// (never `design` itself), turned into the [`GearRemapRow`]s the gear-remap
/// confirmation panel shows. `new_indices` comes from a real `apply_edit` call
/// (staying byte-identical to what confirming would actually write), while
/// `non_integral` is computed from the UNROUNDED ratio so it flags a tier whose ideal
/// new position isn't a whole tooth regardless of which `rounding` this preview uses.
pub(super) fn gear_remap_preview(
    design: &Design,
    from_gear: i32,
    to_gear: i32,
    rounding: RemapRounding,
) -> Vec<GearRemapRow> {
    let format_indices = |v: &[f64]| v.iter().map(f64::to_string).collect::<Vec<_>>().join(", ");
    let original: Vec<String> = design
        .tiers
        .iter()
        .map(|t| format_indices(&t.indices))
        .collect();
    // Same magnitude-only ratio `Edit::RemapIndices` itself applies -- computing it
    // here from the signed tooth counts is what made this preview disagree with the
    // edit for a design whose `.asc` header carries a negative `g`.
    let ratio = remap_ratio(from_gear, to_gear);
    let non_integral: Vec<bool> = design
        .tiers
        .iter()
        .map(|t| t.indices.iter().any(|&i| (i * ratio).fract() != 0.0))
        .collect();

    let mut remapped = design.clone();
    if remapped
        .apply_edit(Edit::RemapIndices {
            from_gear,
            to_gear,
            rounding,
        })
        .is_err()
    {
        // `RemapIndices` names no tier index, so `apply_edit` cannot fail for it.
        // Defensive only: an empty preview reads as "nothing to remap" rather than
        // panicking on a future change to that contract.
        return Vec::new();
    }

    design
        .tiers
        .iter()
        .zip(&remapped.tiers)
        .enumerate()
        .map(|(i, (before, after))| GearRemapRow {
            name: before.name.clone().into(),
            old_indices: original[i].clone().into(),
            new_indices: format_indices(&after.indices).into(),
            non_integral: non_integral[i],
        })
        .collect()
}

/// The `key` [`EditorState::apply_coalescing`] passes through to
/// [`indicatrix_cut_core::History::apply_coalescing`] for an angle nudge targeting
/// exactly `targets` (a single row, or every row in a multi-select group) --
/// order-independent (sorts first) so nudging tiers `{3, 4}` always hashes the same
/// regardless of which one the wheel/keyboard event actually fired on, but distinct
/// from nudging tier `3` alone: a different target SET must never coalesce with a
/// nudge of a different one, even when they overlap.
pub(super) fn angle_nudge_coalesce_key(targets: &[usize]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut sorted = targets.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // Hashes each element via an explicit loop, not `sorted.hash(&mut hasher)` on the
    // whole `Vec` -- clippy::nursery's `collection_is_never_read` does not recognize
    // a `Hash` call on the whole collection as "reading" it (it only ever mutates
    // `sorted` via `sort_unstable`/`dedup` from its point of view otherwise) and
    // flags it as dead, even though hashing every element genuinely does read the
    // sorted, deduplicated contents this function exists to key on.
    sorted.len().hash(&mut hasher);
    for value in &sorted {
        value.hash(&mut hasher);
    }
    hasher.finish()
}

// Colocated with the two functions they cover instead of added to `tests.rs`
// below, keeping these small helper tests next to the implementation they
// exercise.
#[cfg(test)]
mod inline_tests {
    use super::{
        BTreeSet, EditorState, OrbitUnit, apply_proposed_angles, design_material_index_from_name,
        design_material_name_from_index, design_material_options, external_proportions_note,
        girdle_and_ratio_texts, orbit_status_text, preform_y_offset_mm_text, tier_items_stale,
        tier_items_stale_with_last_solved,
    };
    use indicatrix::{geometry::stone_metrics::SolidMetrics, optics::materials::GemMaterial};
    use indicatrix_cut_core::{Design, PreformSpec};

    fn unit(members: usize, expected_len: usize) -> OrbitUnit {
        OrbitUnit {
            members: (0..members).map(|i| i as f64).collect(),
            expected_len,
        }
    }

    #[test]
    fn orbit_status_text_reports_a_clean_multi_unit_fold_as_complete() {
        let (text, incomplete) = orbit_status_text(&[unit(4, 4), unit(4, 4)]);
        assert_eq!(text, "2 orbits");
        assert!(!incomplete);
    }

    #[test]
    fn orbit_status_text_counts_how_many_units_are_incomplete() {
        let (text, incomplete) = orbit_status_text(&[unit(4, 4), unit(2, 4), unit(4, 4)]);
        assert_eq!(text, "3 orbits (1 incomplete)");
        assert!(incomplete);
    }

    #[test]
    fn orbit_status_text_reports_a_mixed_fold_with_no_complete_unit_as_not_symmetric() {
        // `orbit::mod`'s own corpus doc comment's `mixed_fold` example: several
        // units, but every one of them short a member -- genuine incoherence,
        // not a single benign partial occurrence.
        let (text, incomplete) = orbit_status_text(&[unit(2, 4), unit(4, 8)]);
        assert_eq!(text, "not symmetric");
        assert!(incomplete);
    }

    fn metrics(
        width_axis: f64,
        length_axis: f64,
        total_height: f64,
        crown_height: Option<f64>,
        pavilion_depth: Option<f64>,
    ) -> SolidMetrics {
        SolidMetrics {
            volume: 1.0,
            width_axis,
            length_axis,
            width_caliper: width_axis,
            length_caliper: length_axis,
            total_height,
            crown_height,
            pavilion_depth,
            girdle_thickness: None,
            vertex_count: 8,
        }
    }

    #[test]
    fn external_proportions_note_reports_lw_hw_cw_pw_when_a_girdle_is_present() {
        let m = metrics(2.0, 2.0, 1.2, Some(0.4), Some(0.6));
        let note = external_proportions_note(&m, None);
        assert!(note.contains("L/W 1.000"));
        assert!(note.contains("H/W 0.600"));
        assert!(note.contains("C/W 0.200"));
        assert!(note.contains("P/W 0.300"));
    }

    #[test]
    fn external_proportions_note_omits_cw_pw_without_a_live_girdle_facet() {
        let m = metrics(2.0, 2.0, 1.2, None, None);
        let note = external_proportions_note(&m, None);
        assert!(note.contains("L/W"));
        assert!(note.contains("H/W"));
        assert!(!note.contains("C/W"));
        assert!(!note.contains("P/W"));
    }

    #[test]
    fn external_proportions_note_is_empty_for_a_zero_width_solid() {
        let m = metrics(0.0, 2.0, 1.2, None, None);
        assert_eq!(external_proportions_note(&m, None).len(), 0);
    }

    #[test]
    fn external_proportions_note_appends_absolute_mm_when_a_scale_is_known() {
        // Once a girdle diameter anchors a real scale, the
        // banner should show absolute size alongside the dimensionless ratios --
        // GemCad/GCS always show both, and the ratios alone still leave "how big
        // is it really" unanswered.
        let m = metrics(2.0, 2.0, 1.2, None, None);
        let note = external_proportions_note(&m, Some(2.5));
        assert!(note.contains("5.00 x 3.00 mm"), "note was: {note}");
    }

    #[test]
    fn external_proportions_note_omits_mm_clause_without_a_known_scale() {
        let m = metrics(2.0, 2.0, 1.2, None, None);
        let note = external_proportions_note(&m, None);
        assert!(!note.contains("mm"));
    }

    // --- girdle_and_ratio_texts ---

    fn fixture_design() -> Design {
        Design::fresh(PreformSpec::cylinder(96, 1.5, 1.0, 1.5), 96, 8, 1.54)
    }

    #[test]
    fn girdle_and_ratio_texts_dashes_out_a_design_with_no_tiers() {
        // A brand-new design (`Design::fresh`) has no tiers at all -- it still
        // SOLVES (an empty mast list is a valid, closed, zero-plane solve), so
        // only the explicit `tiers.is_empty()` guard, not a `Design::solve`
        // failure, is what stops `stone_proportions` from measuring the bare
        // preform block. Every one of the four figures must read "-", the same
        // fallback `proportions_texts` uses, never a stale zero.
        let design = fixture_design();
        assert_eq!(
            girdle_and_ratio_texts(&design),
            (
                "-".to_string(),
                "-".to_string(),
                "-".to_string(),
                "-".to_string()
            )
        );
    }

    // --- design_material_options / design_material_name_from_index (a custom
    // material colliding with a built-in name is shown, not silently hidden
    // from this combo) ---

    #[test]
    fn design_material_options_lists_a_builtin_colliding_custom_material_under_a_suffixed_label() {
        let mut custom = GemMaterial::diamond();
        custom.name = "Diamond".to_string();
        let options = design_material_options(std::slice::from_ref(&custom));
        assert!(
            options.iter().any(|o| o == "Diamond (custom)"),
            "options was: {options:?}"
        );
        // The built-in entry itself must still be present too -- this is an
        // addition, not a replacement.
        assert!(options.iter().any(|o| o == "Diamond"));
    }

    /// A vault with no custom materials must still yield
    /// the built-in list on the FIRST call -- an empty incoming name list must
    /// not be mistaken for "cache already built", which would hand back an
    /// empty combo model.
    #[test]
    fn material_combo_options_with_no_custom_materials_lists_the_builtins_on_first_call() {
        let state = EditorState::fresh();
        let options = state.material_combo_options(&[]);
        assert_eq!(options, design_material_options(&[]));
        assert_eq!(options.first().map(String::as_str), Some("(none)"));
        assert!(
            options.iter().any(|o| o == "Diamond"),
            "options was: {options:?}"
        );
        // Second call with the same (empty) custom list is served from the cache.
        assert_eq!(state.material_combo_options(&[]), options);
    }

    #[test]
    fn design_material_options_lists_a_non_colliding_custom_material_plainly() {
        let mut custom = GemMaterial::diamond();
        custom.name = "My Garnet".to_string();
        let options = design_material_options(std::slice::from_ref(&custom));
        assert!(options.iter().any(|o| o == "My Garnet"));
        assert!(!options.iter().any(|o| o.contains("(custom)")));
    }

    #[test]
    fn design_material_name_from_index_strips_the_collision_suffix() {
        let mut custom = GemMaterial::diamond();
        custom.name = "Diamond".to_string();
        let options = design_material_options(std::slice::from_ref(&custom));
        let index = options
            .iter()
            .position(|o| o == "Diamond (custom)")
            .expect("labeled entry must exist");
        assert_eq!(
            design_material_name_from_index(index as i32, &options),
            Some("Diamond".to_string()),
            "the parsed name must be the real material name, not the display label, \
             so it still resolves through EditorMaterialLookup's custom-over-built-in \
             precedence"
        );
    }

    #[test]
    fn design_material_index_from_name_still_finds_the_plain_builtin_entry() {
        let mut custom = GemMaterial::diamond();
        custom.name = "Diamond".to_string();
        let options = design_material_options(std::slice::from_ref(&custom));
        // `builtin_preset_names`' own "Diamond" entry comes first in the list, so
        // a plain lookup by name must still resolve to it, not the labeled
        // duplicate further down.
        assert_eq!(
            design_material_index_from_name(Some("Diamond"), &options),
            1
        );
    }

    // --- tier_items_stale_with_last_solved ---

    #[test]
    fn tier_items_stale_with_last_solved_falls_back_to_dashes_with_no_cached_solve() {
        let design = fixture_design();
        let rows = tier_items_stale_with_last_solved(&design, 1.54, None, &BTreeSet::new());
        assert!(rows.is_empty(), "a fresh design has no tiers to show");
    }

    fn scale_reference_tier(name: &str) -> indicatrix_cut_core::ConstraintTier {
        indicatrix_cut_core::ConstraintTier {
            angle_deg: 40.0,
            name: name.to_string(),
            indices: Vec::new(),
            constraint: indicatrix::geometry::meet_solver::MeetConstraint::ScaleReference(1.0),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    fn solved_tier(mast: f64) -> super::SolvedTier {
        super::SolvedTier {
            mast,
            strategy: indicatrix::geometry::meet_solver::SolveStrategy::ScaleReference,
            detail: "given (scale reference)".to_string(),
        }
    }

    /// A tier the edit itself touched (in `dirty`) must fall back to `"-"`/"not
    /// solved" -- no previous mast can be trusted for it. A tier the edit left
    /// alone must keep its previous mast, tagged "stale" rather than shown as a
    /// fresh solve.
    #[test]
    fn tier_items_stale_with_last_solved_blanks_only_the_dirty_rows() {
        let mut design = fixture_design();
        design.tiers = vec![scale_reference_tier("Table"), scale_reference_tier("P1")];
        let last_solved = vec![solved_tier(1.0), solved_tier(0.75)];
        let dirty = BTreeSet::from([0]);

        let rows = tier_items_stale_with_last_solved(&design, 1.54, Some(&last_solved), &dirty);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].mast.as_str(), "-");
        assert_eq!(rows[0].strategy.as_str(), "not solved");
        assert_eq!(rows[1].mast.as_str(), "0.7500");
        assert_eq!(rows[1].strategy.as_str(), "stale (Scale reference)");
    }

    /// A tier count mismatch (a tier was added/removed since `last_solved` was
    /// captured) must not be trusted positionally -- every row falls back to the
    /// same blank treatment as no cached solve at all.
    #[test]
    fn tier_items_stale_with_last_solved_ignores_a_mismatched_tier_count() {
        let mut design = fixture_design();
        design.tiers = vec![scale_reference_tier("Table"), scale_reference_tier("P1")];
        let stale_last_solved = vec![solved_tier(1.0)];
        let rows = tier_items_stale_with_last_solved(
            &design,
            1.54,
            Some(&stale_last_solved),
            &BTreeSet::new(),
        );
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.mast.as_str() == "-"));
    }

    // --- preform_y_offset_mm_text ---

    #[test]
    fn preform_y_offset_mm_text_is_empty_with_no_scale() {
        assert_eq!(preform_y_offset_mm_text(0.3, None), "");
    }

    #[test]
    fn preform_y_offset_mm_text_converts_through_mm_per_unit() {
        assert_eq!(preform_y_offset_mm_text(0.3, Some(4.0)), "1.20");
    }

    // --- apply_proposed_angles ---

    #[test]
    fn apply_proposed_angles_patches_only_the_named_rows() {
        let mut design = fixture_design();
        design.tiers = vec![scale_reference_tier("Table"), scale_reference_tier("P1")];
        let mut rows = tier_items_stale(&design, 1.54);
        let changes = vec![indicatrix_cut_core::AngleChange {
            index: 1,
            from_deg: 40.0,
            to_deg: 41.25,
        }];
        apply_proposed_angles(&mut rows, &changes);
        assert_eq!(rows[0].proposed_angle.as_str(), "");
        assert_eq!(rows[1].proposed_angle.as_str(), "41.25");
    }

    #[test]
    fn apply_proposed_angles_ignores_an_out_of_range_index() {
        let design = fixture_design();
        let mut rows = tier_items_stale(&design, 1.54);
        let changes = vec![indicatrix_cut_core::AngleChange {
            index: 5,
            from_deg: 0.0,
            to_deg: 12.0,
        }];
        // A tierless design's row list is empty -- this must not panic.
        apply_proposed_angles(&mut rows, &changes);
        assert_eq!(rows.len(), 0);
    }
}

#[cfg(test)]
mod tests;
