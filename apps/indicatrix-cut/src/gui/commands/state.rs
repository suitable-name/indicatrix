//! A plain snapshot of everything that decides whether a command can run right now.
//!
//! The palette never asks the window directly whether a command is available. It
//! captures one [`CommandState`] (a set of true-or-false [`Fact`]s) and the rules in
//! `rules` read only that. This keeps every rule a pure function that tests can feed with
//! a hand-built state, with no window and no Slint involved.

use crate::{
    BatchModel, EditorModel, GuideModel, LibraryModel, MainWindow, PreferencesModel,
    SolidPreviewModel,
};
use slint::ComponentHandle;
use std::collections::HashSet;

/// One group of controls the worked-example guide can lock.
///
/// The names follow `GuideAllow` in `ui/models/guide.slint` and the
/// `GuideModel.allows_*()` helper each group is read through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GuideGroup {
    /// New Design... (command bar, File menu, Ctrl+N).
    NewDesign,
    /// Add Tier and the inspector's Tier tab.
    TierForm,
    /// The tier table's toolbar and the command bar's tier buttons.
    TierTable,
    /// The Design Settings panel.
    DesignSettings,
    /// Solve, F5 and the auto-solve budget.
    Solve,
    /// The inspector's Preform tab.
    PreformTab,
    /// Deep Solve, Optimize, Retarget, Snapshot, Compare, Tilt Curves and Preferences.
    Advanced,
    /// Live Render, the Cutting Instructions tab and the Files tab.
    ViewTabs,
    /// Load, Open, Save and every export.
    FileOps,
    /// Undo and Redo.
    History,
    /// Adding and editing concave tiers.
    ConcaveTier,
}

impl GuideGroup {
    /// Every group, in the order of `GuideAllow`.
    pub const ALL: [Self; 11] = [
        Self::NewDesign,
        Self::TierForm,
        Self::TierTable,
        Self::DesignSettings,
        Self::Solve,
        Self::PreformTab,
        Self::Advanced,
        Self::ViewTabs,
        Self::FileOps,
        Self::History,
        Self::ConcaveTier,
    ];
}

/// One true-or-false statement about the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fact {
    /// The editor is available (`EditorModel.enabled`).
    Editor,
    /// A real design is open (`EditorModel.has_design`).
    Design,
    /// A tier is selected in the tier table.
    TierSelected,
    /// There is an edit to undo.
    CanUndo,
    /// There is an edit to redo.
    CanRedo,
    /// Solve, Deep Solve or Optimize is running (`EditorModel.busy_action` is not empty).
    Busy,
    /// A normal Solve is running.
    SolveRunning,
    /// Deep Solve is running.
    DeepSolveRunning,
    /// Optimize is running.
    OptimizeRunning,
    /// Deep Solve can start (it has printed proportions and a tier to repair).
    DeepSolveAvailable,
    /// Optimize can start (it has a free tier to move).
    OptimizeAvailable,
    /// A snapshot was taken this session.
    HasSnapshot,
    /// The design has been saved or exported at least once this session.
    HasLastSaved,
    /// A design is selected in the library list.
    LibrarySelected,
    /// The preview-image batch is running.
    PreviewBatchRunning,
    /// The tilt-curve batch is running.
    TiltBatchRunning,
    /// The Simple interface is on (otherwise Advanced).
    SimpleInterface,
    /// The 3D tab is showing the Edit view.
    EditView,
    /// The Edit view's viewport is in Diagram mode.
    DiagramView,
    /// The guide leaves this group of controls usable (always true while it is closed).
    Allows(GuideGroup),
}

/// The facts that hold right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandState {
    facts: HashSet<Fact>,
}

impl CommandState {
    /// Whether `fact` holds.
    #[must_use]
    pub fn has(&self, fact: Fact) -> bool {
        self.facts.contains(&fact)
    }

    /// Records that `fact` holds when `on` is true, and that it does not otherwise.
    pub fn set(&mut self, fact: Fact, on: bool) {
        if on {
            self.facts.insert(fact);
        } else {
            self.facts.remove(&fact);
        }
    }

    /// The same state with every guide lock lifted: what a command sees when the guide runs it
    /// for the learner (Next on an unfinished step), which is the app acting, not the learner
    /// clicking a control the step has locked.
    #[must_use]
    pub fn without_guide_locks(mut self) -> Self {
        for group in GuideGroup::ALL {
            self.set(Fact::Allows(group), true);
        }
        self
    }

    /// Reads the current state of `ui`. Cheap: a few dozen property reads, no borrow of the
    /// editor state, so it is safe to call from anywhere on the UI thread.
    #[must_use]
    pub fn capture(ui: &MainWindow) -> Self {
        let editor = ui.global::<EditorModel>();
        let guide = ui.global::<GuideModel>();
        let batch = ui.global::<BatchModel>();
        let busy = !editor.get_busy_action().is_empty();
        let mut state = Self::default();

        state.set(Fact::Editor, editor.get_enabled());
        state.set(Fact::Design, editor.get_has_design());
        state.set(Fact::TierSelected, editor.get_selected_tier_index() >= 0);
        state.set(Fact::CanUndo, editor.get_can_undo());
        state.set(Fact::CanRedo, editor.get_can_redo());
        state.set(Fact::Busy, busy);
        state.set(Fact::SolveRunning, editor.get_solve_running());
        state.set(Fact::DeepSolveRunning, editor.get_deep_solve_running());
        state.set(Fact::OptimizeRunning, editor.get_optimize_running());
        state.set(Fact::DeepSolveAvailable, editor.get_deep_solve_available());
        state.set(Fact::OptimizeAvailable, editor.get_optimize_available());
        state.set(Fact::HasSnapshot, editor.get_has_snapshot());
        state.set(Fact::HasLastSaved, !editor.get_last_saved_path().is_empty());
        state.set(
            Fact::LibrarySelected,
            ui.global::<LibraryModel>().get_selected_entry_id() >= 0,
        );
        state.set(Fact::PreviewBatchRunning, batch.get_preview_batch_running());
        state.set(Fact::TiltBatchRunning, batch.get_tilt_batch_running());
        state.set(
            Fact::SimpleInterface,
            ui.global::<PreferencesModel>().get_simple_mode(),
        );
        state.set(
            Fact::EditView,
            ui.get_active_tab() == 0 && ui.get_render_view_tab() == 1,
        );
        state.set(
            Fact::DiagramView,
            ui.global::<SolidPreviewModel>().get_view_mode() == 3,
        );

        // The same `allows_*()` helpers the menu items and buttons are enabled through, so a
        // command is locked exactly when its button is.
        for (group, allowed) in [
            (GuideGroup::NewDesign, guide.invoke_allows_new_design()),
            (GuideGroup::TierForm, guide.invoke_allows_tier_form()),
            (GuideGroup::TierTable, guide.invoke_allows_tier_table()),
            (
                GuideGroup::DesignSettings,
                guide.invoke_allows_design_settings(),
            ),
            (GuideGroup::Solve, guide.invoke_allows_solve()),
            (GuideGroup::PreformTab, guide.invoke_allows_preform_tab()),
            (GuideGroup::Advanced, guide.invoke_allows_advanced()),
            (GuideGroup::ViewTabs, guide.invoke_allows_view_tabs()),
            (GuideGroup::FileOps, guide.invoke_allows_file_ops()),
            (GuideGroup::History, guide.invoke_allows_history()),
            (GuideGroup::ConcaveTier, guide.invoke_allows_concave_tier()),
        ] {
            state.set(Fact::Allows(group), allowed);
        }
        state
    }
}

#[cfg(test)]
impl CommandState {
    /// A state in which every ordinary command is available: a design is open with a tier
    /// selected, nothing is running, the guide is closed and the Edit view is showing.
    /// "Running" facts are on so the cancel commands are available too.
    #[must_use]
    pub fn ready() -> Self {
        let mut state = Self::default();
        for fact in [
            Fact::Editor,
            Fact::Design,
            Fact::TierSelected,
            Fact::CanUndo,
            Fact::CanRedo,
            Fact::SolveRunning,
            Fact::DeepSolveRunning,
            Fact::OptimizeRunning,
            Fact::DeepSolveAvailable,
            Fact::OptimizeAvailable,
            Fact::HasSnapshot,
            Fact::HasLastSaved,
            Fact::LibrarySelected,
            Fact::EditView,
        ] {
            state.set(fact, true);
        }
        for group in GuideGroup::ALL {
            state.set(Fact::Allows(group), true);
        }
        state
    }

    /// The same state with `fact` switched on or off.
    #[must_use]
    pub fn with(mut self, fact: Fact, on: bool) -> Self {
        self.set(fact, on);
        self
    }
}
