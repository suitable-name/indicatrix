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

use crate::{EditorTierItem, GearRemapRow};
use indicatrix::{
    geometry::{
        GpuFacetPlane,
        meet_solver::{MeetConstraint, SolveStrategy},
        stone_metrics::SolidStatus,
    },
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    Design, Edit, EditError, FreshDesignSpec, History, MaterialSelection, OptimizeOutcome,
    OrbitUnit, RemapRounding, Risk, tier_margin_deg, windowing_risk,
};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
};

/// The gear combo's fixed pill choices, before the combo's own trailing "Custom"
/// entry -- shared by the design settings panel's gear control and the new-design
/// dialog (`super::loading::parse_new_design_form`) so both present the same list.
pub(super) const GEAR_PRESETS: [i32; 6] = [96, 80, 77, 72, 64, 120];

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
    /// The paired `.asc`'s bare file name and exact original text, when this design's
    /// schedule came from a real `.asc` file on disk (a catalogue attachment, or a
    /// previous native save/open). `None` for a brand-new design or one reconstructed
    /// from the angle-table placeholder. Fed to [`indicatrix_cut_core::save_paired`]'s
    /// `original_asc_text` parameter, which lets a native Save leave the `.asc` half
    /// byte-for-byte untouched instead of regenerating it.
    pub(super) asc_filename: Option<String>,
    pub(super) original_asc_text: Option<String>,
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
}

impl EditorState {
    /// A brand-new design: a generously sized cylindrical preform and an empty
    /// schedule, matching the "New" button's job -- start from something that already
    /// renders as a real, closed stone rather than an empty viewport.
    pub(super) fn fresh() -> Self {
        let preform = indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5);
        Self {
            design: Design::fresh(preform, 96, 8, 1.54),
            history: History::new(),
            printed_proportions: None,
            generation: Arc::new(AtomicU64::new(0)),
            deep_solve: None,
            optimize: None,
            pending_optimize: Arc::new(Mutex::new(None)),
            asc_filename: None,
            original_asc_text: None,
            pending_gear_remap: None,
            pending_retarget: None,
            multi_selected: BTreeSet::new(),
        }
    }

    /// A brand-new design from the New Design dialog's full [`FreshDesignSpec`]
    /// (preform, gear, symmetry, mirror, starting material). Otherwise identical to
    /// [`Self::fresh`]: empty `History`, no printed proportions, no pending work.
    pub(super) fn fresh_from_spec(spec: FreshDesignSpec) -> Self {
        Self {
            design: Design::fresh_from_spec(spec),
            history: History::new(),
            printed_proportions: None,
            generation: Arc::new(AtomicU64::new(0)),
            deep_solve: None,
            optimize: None,
            pending_optimize: Arc::new(Mutex::new(None)),
            asc_filename: None,
            original_asc_text: None,
            pending_gear_remap: None,
            pending_retarget: None,
            multi_selected: BTreeSet::new(),
        }
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
    /// since `History::undo` itself no longer panics on this path.
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
}

/// Splits a [`MeetConstraint`] into the `(constraint_kind, constraint_text)` pair
/// [`EditorTierItem`] carries and the tier-edit form round-trips through its
/// `LineEdit` -- the inverse of `super::loading::parse_tier_form`'s constraint parsing.
fn constraint_kind_and_text(constraint: &MeetConstraint) -> (i32, String) {
    match constraint {
        MeetConstraint::MeetExisting => (0, String::new()),
        MeetConstraint::MeetNamed(names) => (1, names.join(", ")),
        // Rust's shortest round-trippable `f64` Display, not a fixed decimal count --
        // this feeds back into the form's `LineEdit`, so it must read back exactly
        // what `solve` (or the user) produced.
        MeetConstraint::ScaleReference(value) => (2, value.to_string()),
    }
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
        many => {
            let incomplete = many.iter().any(|u| !u.is_complete());
            (format!("{} orbits", many.len()), incomplete)
        }
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

/// [`EditorTierItem::margin_text`]/`risk_level` for one tier. Meaningful for a
/// pavilion tier specifically (`tier_angle_deg < 0.0`) -- a crown or girdle tier
/// always reads `("", -1)`, "nothing to show," not a wrong badge. `n_d` is the
/// design's effective refractive index, the same value the design settings panel's
/// RI/critical-angle readouts show.
fn tier_margin_and_risk(tier_angle_deg: f64, n_d: f64) -> (String, i32) {
    if tier_angle_deg >= 0.0 {
        return (String::new(), -1);
    }
    let margin = tier_margin_deg(tier_angle_deg, n_d);
    let risk_level = match windowing_risk(tier_angle_deg, n_d) {
        Risk::Safe => 0,
        Risk::Marginal => 1,
        Risk::Windows => 2,
    };
    (format!("{margin:+.1}\u{b0}"), risk_level)
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

/// Builds the tier list's rows WITHOUT calling [`Design::solve`] at all -- every
/// mast/strategy cell reads `"-"`/`"not solved"` (flagged uncertain) regardless of
/// what the design actually is. Used by `refresh_editor_panel_stale` after every edit
/// that isn't the explicit "Solve" action -- see this group's `mod.rs` doc comment for
/// why: a real design can take multiple seconds to solve, and showing a PREVIOUS
/// solve's masts would be actively wrong the moment the edit changed tier count/order.
pub(super) fn tier_items_stale(design: &Design, n_d: f64) -> Vec<EditorTierItem> {
    design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let (constraint_kind, constraint_text) = constraint_kind_and_text(&tier.constraint);
            let units = indicatrix_cut_core::orbit_units(&tier.indices, &design.meta);
            let (orbit_status, orbit_incomplete) = orbit_status_text(&units);
            let (margin_text, risk_level) = tier_margin_and_risk(tier.angle_deg, n_d);
            EditorTierItem {
                index: index as i32,
                angle_deg: tier.angle_deg.to_string().into(),
                name: tier.name.clone().into(),
                indices: tier
                    .indices
                    .iter()
                    .map(f64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into(),
                constraint_kind,
                constraint_text: constraint_text.into(),
                mast: "-".into(),
                strategy: "not solved".into(),
                strategy_is_uncertain: true,
                imported_meet_text: imported_meet_text(tier.imported_meet.as_ref()).into(),
                orbit_status: orbit_status.into(),
                orbit_incomplete,
                is_detached: !tier.detached.is_empty(),
                margin_text: margin_text.into(),
                risk_level,
                // Always `false` here -- this builder rebuilds the WHOLE tier list
                // from scratch with no access to `EditorState::multi_selected`
                // (deliberately not a parameter here; see `apply_multi_selection`'s
                // own doc comment for why that's a post-pass instead). Every real
                // push site (`view::refresh_editor_panel`/`push_stale_content`,
                // `auto_solve`'s background-solve completion) calls
                // `apply_multi_selection` on the built `Vec` immediately afterward,
                // so the live highlight survives a full refresh (an edit, undo/redo,
                // Solve) rather than resetting on every one.
                multi_selected: false,
            }
        })
        .collect()
}

/// Converts `design`'s current tier list into the rows `EditorView`'s list renders,
/// including the solved mast and [`SolveStrategy`] label for each. `index` is the
/// tier's position in `design.tiers` -- round-tripped back by
/// `EditorView.save_tier`/`remove_tier`, a list index rather than a stable id.
///
/// Solves `design` exactly once up front and reports every tier as unsolved (`mast` =
/// `"?"`, strategy flagged uncertain) when [`Design::solve`] returns
/// [`indicatrix_cut_core::MissingAnchor`] -- see [`status_text_and_is_problem`], which
/// surfaces which block(s) are missing an anchor in the validation banner.
///
/// Only called from `refresh_all` (New/Load/the explicit "Solve" action) -- see
/// [`tier_items_stale`] for the no-solve version every other edit callback uses.
pub(super) fn tier_items(design: &Design, n_d: f64) -> Vec<EditorTierItem> {
    let solved = design.solve();
    design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let (constraint_kind, constraint_text) = constraint_kind_and_text(&tier.constraint);
            let (mast, strategy, strategy_is_uncertain) = solved.as_ref().map_or_else(
                |_| ("?".to_string(), "no anchor yet".to_string(), true),
                |rows| {
                    let (label, uncertain) = strategy_label(rows[index].strategy);
                    (rows[index].mast.to_string(), label.to_string(), uncertain)
                },
            );
            let units = indicatrix_cut_core::orbit_units(&tier.indices, &design.meta);
            let (orbit_status, orbit_incomplete) = orbit_status_text(&units);
            let (margin_text, risk_level) = tier_margin_and_risk(tier.angle_deg, n_d);
            EditorTierItem {
                index: index as i32,
                angle_deg: tier.angle_deg.to_string().into(),
                name: tier.name.clone().into(),
                indices: tier
                    .indices
                    .iter()
                    .map(f64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
                    .into(),
                constraint_kind,
                constraint_text: constraint_text.into(),
                mast: mast.into(),
                strategy: strategy.into(),
                strategy_is_uncertain,
                imported_meet_text: imported_meet_text(tier.imported_meet.as_ref()).into(),
                orbit_status: orbit_status.into(),
                orbit_incomplete,
                is_detached: !tier.detached.is_empty(),
                margin_text: margin_text.into(),
                risk_level,
                // Always `false` here -- this builder rebuilds the WHOLE tier list
                // from scratch with no access to `EditorState::multi_selected`
                // (deliberately not a parameter here; see `apply_multi_selection`'s
                // own doc comment for why that's a post-pass instead). Every real
                // push site (`view::refresh_editor_panel`/`push_stale_content`,
                // `auto_solve`'s background-solve completion) calls
                // `apply_multi_selection` on the built `Vec` immediately afterward,
                // so the live highlight survives a full refresh (an edit, undo/redo,
                // Solve) rather than resetting on every one.
                multi_selected: false,
            }
        })
        .collect()
}

/// [`indicatrix_cut_core::manufacturability::check_manufacturability`]'s findings
/// against `design`'s current state, one already-formatted line per warning -- rides
/// along with the explicit "Solve" action rather than running on every edit.
///
/// Solves `design` exactly once and reuses that `Vec<SolvedTier>` for every one of
/// the four checks -- a separate solve from the ones [`tier_items`]/
/// [`status_text_and_is_problem`] each already do, but exactly one solve for all of
/// Phase 4's checks, never a per-check solve.
///
/// `[]` when the design does not currently solve at all ([`indicatrix_cut_core::MissingAnchor`],
/// already surfaced by the validation banner) or [`Design::solve`] succeeds but
/// nothing is found.
pub(super) fn manufacturability_warning_lines(design: &Design) -> Vec<String> {
    let Ok(solved) = design.solve() else {
        return Vec::new();
    };
    indicatrix_cut_core::manufacturability::check_manufacturability(
        design,
        &solved,
        indicatrix_cut_core::manufacturability::DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
    )
    .iter()
    .map(std::string::ToString::to_string)
    .collect()
}

/// The material `ComboBox`'s preset names, in EXACT index order -- index 0 is
/// `"(none)"`; indices 1..=13 are the `GemMaterial::name` strings that also have a
/// `material::built_in_specific_gravity` entry (no Garnet).
///
/// No shared source of truth between Slint and Rust for a `ComboBox`'s `model` list --
/// `editor_view.slint`'s material `ComboBox` literal must be kept in this SAME order
/// by hand; this constant's doc comment is that source of truth in prose.
pub(super) const MATERIAL_PRESET_NAMES: [&str; 14] = [
    "(none)",
    "Diamond",
    "Sapphire",
    "Ruby",
    "Emerald",
    "Zircon",
    "Alexandrite",
    "Topaz",
    "Spinel",
    "Quartz",
    "Tourmaline",
    "Tanzanite",
    "Synthetic Moissanite",
    "Cubic Zirconia",
];

/// [`MATERIAL_PRESET_NAMES`]'s index for `name` (`None` -> `0`, an unrecognized name
/// -> `0` as a safe fallback rather than an out-of-range `ComboBox` index).
pub(super) fn material_index_from_name(name: Option<&str>) -> i32 {
    name.and_then(|n| MATERIAL_PRESET_NAMES.iter().position(|&p| p == n))
        .map_or(0, |i| i as i32)
}

/// The inverse of [`material_index_from_name`]: the name at `index` in
/// [`MATERIAL_PRESET_NAMES`], or `None` for index `0` ("(none)") or an out-of-range
/// index (defensive only -- `EditorView`'s `ComboBox` can never produce one).
pub(super) fn material_name_from_index(index: i32) -> Option<String> {
    usize::try_from(index)
        .ok()
        .and_then(|i| MATERIAL_PRESET_NAMES.get(i))
        .filter(|&&name| name != "(none)")
        .map(|&name| name.to_string())
}

/// Parses the Yield form's three fields into
/// [`indicatrix_cut_core::Edit::SetGirdleDiameterMm`]/[`indicatrix_cut_core::Edit::SetMaterial`]'s
/// payloads. Pure and unit tested directly. An empty `girdle_diameter_mm`/
/// `specific_gravity_override` field parses to `None` (the "unset, use the preset's
/// own figure" state), never an error: blank is a valid, deliberate choice here,
/// unlike the preform's three fields, which always name a real dimension.
pub(super) fn parse_yield_form(
    girdle_diameter_mm: &str,
    material_index: i32,
    specific_gravity_override: &str,
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
        MaterialSelection {
            name: material_name_from_index(material_index),
            specific_gravity_override,
            // No RI-override field in this form -- that lands with the design
            // settings panel instead.
            refractive_index_override: None,
        },
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
pub(super) fn yield_report_texts(design: &Design) -> (String, String, String, String) {
    let Ok(solved) = design.solve() else {
        return (String::new(), String::new(), String::new(), String::new());
    };
    let report = design.yield_report(&solved);

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

/// [`Design::status`], rendered as the Edit tab's validation banner text plus whether
/// it should be styled as a problem (red) or all-clear (green).
///
/// `Unbounded` gets a real message (naming the escaping plane indices) rather than
/// being treated as unreachable: it's "essentially unreachable" through `Design` in
/// practice (the preform always caps every direction), not provably impossible, and a
/// defensive one-line message costs nothing. This function does NOT build any further
/// UI for that case (e.g. highlighting the offending planes) -- no elaborate UI for a
/// state that cannot occur.
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
                .map(|m| format!(" -- volume {:.4}", m.volume))
                .unwrap_or_default();
            (format!("Closed solid.{volume_note}"), false)
        }
        Ok(SolidStatus::Degenerate {
            vertex_count,
            volume,
        }) => {
            let volume_text =
                volume.map_or_else(|| "non-finite".to_string(), |v| format!("{v:.4}"));
            (
                format!(
                    "Degenerate: only {vertex_count} distinct vertex(es), volume {volume_text}."
                ),
                true,
            )
        }
        Ok(SolidStatus::Unbounded { escaping }) => (
            format!("Unbounded: plane(s) {escaping:?} never close the solid."),
            true,
        ),
        // `MissingAnchor` names the block(s) with no scale-reference tier -- the
        // actionable message: "add a Scale Reference tier to the crown/pavilion/girdle".
        Err(missing) => (format!("Cannot solve: {missing}."), true),
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

/// The design settings panel's material combo, in the EXACT order it must list:
/// `MATERIAL_PRESET_NAMES` verbatim, then `custom`'s own names (skipping any that
/// collide case-insensitively with a built-in already listed, since
/// `EditorMaterialLookup` prefers a custom material of the same name over its
/// built-in twin), then a final `"Custom RI…"` sentinel -- see
/// [`design_material_name_from_index`] for what selecting it means. Rust builds this
/// list fresh every refresh (custom materials can change any time) and pushes it
/// straight into `editor_material_combo_options`.
pub(super) fn design_material_options(custom: &[GemMaterial]) -> Vec<String> {
    let mut options: Vec<String> = MATERIAL_PRESET_NAMES
        .iter()
        .map(|&s| s.to_string())
        .collect();
    for material in custom {
        if !options
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&material.name))
        {
            options.push(material.name.clone());
        }
    }
    options.push("Custom RI\u{2026}".to_string());
    options
}

/// The inverse of [`design_material_name_from_index`]: `options`'s index for `name`
/// (case-insensitive), or `0` ("(none)") when `name` is absent or not found.
pub(super) fn design_material_index_from_name(name: Option<&str>, options: &[String]) -> i32 {
    name.and_then(|n| options.iter().position(|o| o.eq_ignore_ascii_case(n)))
        .map_or(0, |i| i as i32)
}

/// The name at `index` in `options`, or `None` for index `0` ("(none)"), the trailing
/// `"Custom RI…"` sentinel, or an out-of-range index. Selecting `"Custom RI…"`
/// clears `MaterialSelection::name` the same way "(none)" does -- it exists so a
/// species with no built-in or catalogue preset (e.g. garnet) can still be given a
/// real, typed refractive index without pretending to pick a preset that isn't used.
pub(super) fn design_material_name_from_index(index: i32, options: &[String]) -> Option<String> {
    usize::try_from(index)
        .ok()
        .and_then(|i| options.get(i))
        .filter(|&name| name != "(none)" && name != "Custom RI\u{2026}")
        .cloned()
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
    let non_integral: Vec<bool> = design
        .tiers
        .iter()
        .map(|t| {
            t.indices.iter().any(|&i| {
                let ratio = if from_gear == 0 {
                    1.0
                } else {
                    f64::from(to_gear) / f64::from(from_gear)
                };
                (i * ratio).fract() != 0.0
            })
        })
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

#[cfg(test)]
mod tests;
