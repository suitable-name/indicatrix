//! The worked-example walkthrough: pushing its step content
//! (`indicatrix_editor::guide::STEPS`, shared with the web app) into
//! `ui/models/guide.slint`'s `GuideModel` once at startup ([`setup_guide`]), and
//! advancing it automatically once the design reaches the current step's goal
//! ([`progress`], over `indicatrix_editor::guide::goal_reached`).
//!
//! The Slint side owns navigation (`start`/`next`/`back`/`close`, the short "Done"
//! moment before an automatic advance) and every lock: each control ANDs a
//! `GuideModel.allows_*()` helper into its own `enabled:`, driven by the current
//! step's `allow` flags this module pushes. Rust's jobs are the content, and
//! reporting reached goals through `GuideModel.notify` -- see
//! [`progress::check_progress`] for where that is called from.

mod progress;

pub(in crate::gui::editor) use indicatrix_editor::guide::NEW_DESIGN_CREATED;
pub(in crate::gui::editor) use progress::{check_design_progress, check_progress, notify};

use super::state::EditorState;
use crate::{GuideAllow, GuideModel, GuideStepData, MainWindow};
use indicatrix_editor::guide::{Group, STEPS, Step};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{cell::RefCell, rc::Rc};

/// `allow`'s groups as the Slint `GuideAllow` struct: a group not listed is locked.
fn guide_allow(allow: &[Group]) -> GuideAllow {
    GuideAllow {
        new_design: allow.contains(&Group::NewDesign),
        tier_form: allow.contains(&Group::TierForm),
        tier_table: allow.contains(&Group::TierTable),
        design_settings: allow.contains(&Group::DesignSettings),
        solve: allow.contains(&Group::Solve),
        preform_tab: allow.contains(&Group::PreformTab),
        advanced: allow.contains(&Group::Advanced),
        view_tabs: allow.contains(&Group::ViewTabs),
        file_ops: allow.contains(&Group::FileOps),
        history: allow.contains(&Group::History),
    }
}

/// One [`Step`] as the Slint `GuideStepData` the panel renders.
fn step_data(step: &Step) -> GuideStepData {
    let actions: Vec<SharedString> = step
        .actions
        .iter()
        .map(|a| SharedString::from(*a))
        .collect();
    GuideStepData {
        title: step.title.into(),
        intro: step.intro.into(),
        actions: ModelRc::new(VecModel::from(actions)),
        check: step.check.into(),
        why: step.why.into(),
        waiting: step.waiting.into(),
        highlight_target: step.highlight_target.into(),
        completion: step.completion.into(),
        allow: guide_allow(step.allow),
    }
}

/// Pushes [`STEPS`] into `GuideModel.steps` once, at startup, and registers
/// `GuideModel.step_entered`: every time a step is entered going forward, its goal
/// is checked straight away against the current design, so a goal that is already
/// met (auto-solve finished before the user got there, say) completes without
/// waiting for another edit. Called from `gui::editor::setup_editor_callbacks`.
pub(in crate::gui::editor) fn setup_guide(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    let steps: Vec<GuideStepData> = STEPS.iter().map(step_data).collect();
    ui.global::<GuideModel>()
        .set_steps(ModelRc::new(VecModel::from(steps)));

    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<GuideModel>().on_step_entered(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        // A writer already holding the state refreshes on its way out, and every
        // refresh ends in `check_progress` -- skipping here loses nothing.
        let Ok(st) = state.try_borrow() else {
            return;
        };
        check_progress(&ui, &st);
    });
}
