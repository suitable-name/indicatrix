//! The guided walkthrough (Help > Guided walkthrough): pushing its step content into
//! `ui/models/guide_model.slint`'s `GuideModel`, advancing it automatically when the design
//! reaches the current step's goal, and keeping the reader's place across a reload.
//!
//! # Shared with the desktop
//!
//! The ten steps, the control groups each leaves usable and the goal predicates are
//! `indicatrix_editor::guide` (`STEPS`, `goal_reached` through `reached_completion`), the
//! same code the desktop's `gui/editor/guide` pushes into its own `GuideModel`. The Slint
//! side owns navigation (`start` / `next` / `back` / `close`, the short "Done" moment
//! before an automatic advance) and every lock: each control ANDs a
//! `GuideModel.allows-*()` helper into its own `enabled`, driven by the current step's
//! `allow` flags [`wire`] pushes. Rust's jobs are the content, reporting reached goals
//! through `GuideModel.notify`, and the tab session.
//!
//! # Which action reports what
//!
//! Goals are judged from STATE, never from which button was clicked, so every route to a
//! goal counts (quick add, the inspector form, an inline edit, undo / redo, a drag, the
//! auto-solve), and a rejected form -- which never changes the design -- can never
//! advance. [`check_progress`] is the one evaluation; it runs
//!
//! - at the end of every edit ([`super::edit::finish_edit`]: tier added / edited / removed,
//!   Apply Material, Apply Yield Inputs, orbit tools, Optimize / Retarget applied, ...);
//! - after an undo or redo, and after a design is opened or restored;
//! - when a solve finishes (`app::solve::finish`: "Solve and check", whether the button or
//!   the auto-solve ran it);
//! - whenever the tier table sees the design or its solve change (its 100 ms poll: a drag
//!   in the Solid view, anything not listed above);
//! - when a step is entered going forward (`GuideModel.step-entered`), so a goal that is
//!   already met completes without waiting for another edit.
//!
//! The one goal that is an EVENT rather than a state, a New Design created
//! (`NEW_DESIGN_CREATED`), is reported by [`notify`] from the two places a design is made
//! from a template: the New Design dialog and the gallery's `AppModel.new-design`.
//!
//! # Tab session
//!
//! Open / step / collapsed / the dragged panel position are stored as JSON under
//! `sessionStorage["indicatrix.guide.v1"]` (`indicatrix_web_core::guide::GuideProgress`),
//! written a moment after any of them changes and read back by [`restore`].

use crate::{
    GuideAllow, GuideModel, GuideStepData,
    app::{
        Ctx, persist,
        state::{SolveState, WebApp},
    },
};
use indicatrix_editor::guide::{Group, STEPS, Step, reached_completion};
use indicatrix_web_core::guide::{GUIDE_KEY, GuideProgress, web_wording};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::time::Duration;

/// Quiet time before a change of the guide's state is written.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(300);

thread_local! {
    /// Debounces the `sessionStorage` write (restarting the same timer is what makes it
    /// a debounce).
    static SAVE: Timer = Timer::default();
}

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
    // The desktop's few lines that name a desktop-only place are reworded for the browser
    // (`indicatrix_web_core::guide::WEB_WORDING`); the steps, locks and goals are shared.
    let actions: Vec<SharedString> = step
        .actions
        .iter()
        .map(|a| SharedString::from(web_wording(a)))
        .collect();
    GuideStepData {
        title: step.title.into(),
        intro: web_wording(step.intro).into(),
        actions: ModelRc::new(VecModel::from(actions)),
        check: web_wording(step.check).into(),
        why: web_wording(step.why).into(),
        waiting: step.waiting.into(),
        highlight_target: step.highlight_target.into(),
        completion: step.completion.into(),
        allow: guide_allow(step.allow),
    }
}

/// Whether the design's current solve is a closed solid: the editor's own "solved"
/// verdict that is not a problem (`solved_closed` in `guide::goal_reached`).
fn solved_closed(app: &WebApp) -> bool {
    app.current_solved().is_some() && matches!(app.solve, SolveState::Solved { problem: false, .. })
}

/// Reports `key` to `GuideModel.notify` directly -- for a goal that is an EVENT rather
/// than a state [`check_progress`] could read back afterwards (only
/// `indicatrix_editor::guide::NEW_DESIGN_CREATED` today). The Slint side ignores a key that
/// is not the current step's own.
pub fn notify(ctx: &Ctx, key: &str) {
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<GuideModel>().invoke_notify(key.into());
    }
}

/// Evaluates the current guide step's goal against the design and reports it to
/// `GuideModel.notify` when reached. A no-op while the guide is closed, on a manual step
/// (its goal never reads as reached), or while the current step already shows "Done".
/// Safe to call while something else holds the state (it then does nothing: every writer
/// ends in a refresh that calls it again).
pub fn check_progress(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let guide = ui.global::<GuideModel>();
    if !guide.get_open() || guide.get_step_done() {
        return;
    }
    let Ok(index) = usize::try_from(guide.get_step_index()) else {
        return;
    };
    let key = {
        let Ok(app) = ctx.state.try_borrow() else {
            return;
        };
        let Some(design) = &app.design else {
            return;
        };
        reached_completion(index, &design.session.design, solved_closed(&app))
    };
    if let Some(key) = key {
        guide.invoke_notify(key.into());
    }
}

/// The guide's state as the panel holds it now.
fn progress_of(guide: &GuideModel<'_>) -> GuideProgress {
    GuideProgress {
        open: guide.get_open(),
        step: usize::try_from(guide.get_step_index()).unwrap_or(0),
        collapsed: guide.get_collapsed(),
        float_placed: guide.get_float_placed(),
        float_x: guide.get_float_x(),
        float_y: guide.get_float_y(),
    }
    .sanitized()
}

/// Writes the guide's state to `sessionStorage` after a moment of quiet.
fn schedule_save(ctx: &Ctx) {
    let weak = ctx.ui.clone();
    SAVE.with(|timer| {
        timer.start(TimerMode::SingleShot, SAVE_DEBOUNCE, move || {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            match progress_of(&ui.global::<GuideModel>()).to_json() {
                Ok(json) => persist::write(GUIDE_KEY, &json),
                Err(e) => persist::discard(GUIDE_KEY, &e),
            }
        });
    });
}

/// Puts the stored place back (open, step, collapsed, panel position) and checks the
/// restored step's goal against the restored design. Call once at start-up, after the
/// design has been restored and pushed. An entry that does not parse is removed.
pub fn restore(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let Some(text) = persist::read(GUIDE_KEY) else {
        return;
    };
    match GuideProgress::from_json(&text) {
        Ok(progress) => {
            let guide = ui.global::<GuideModel>();
            guide.set_step_index(i32::try_from(progress.step).unwrap_or(0));
            guide.set_collapsed(progress.collapsed);
            guide.set_float_placed(progress.float_placed);
            guide.set_float_x(progress.float_x);
            guide.set_float_y(progress.float_y);
            guide.set_open(progress.open);
            check_progress(ctx);
        }
        Err(e) => persist::discard(GUIDE_KEY, &e),
    }
}

/// Pushes [`STEPS`] into `GuideModel.steps` once, at start-up, and registers
/// `GuideModel.step-entered` (every time a step is entered going forward, its goal is
/// checked at once) and `GuideModel.state-changed` (the tab-session save).
pub fn wire(ui: &crate::AppWindow, ctx: &Ctx) {
    let guide = ui.global::<GuideModel>();
    let steps: Vec<GuideStepData> = STEPS.iter().map(step_data).collect();
    guide.set_steps(ModelRc::new(VecModel::from(steps)));

    let c = ctx.clone();
    guide.on_step_entered(move || check_progress(&c));
    let c = ctx.clone();
    guide.on_state_changed(move || schedule_save(&c));
}
