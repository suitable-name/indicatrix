//! The UI refresh: small `push_*` functions that copy one slice of [`WebApp`] into
//! `AppModel`, and [`show_message`], the one path every status/error message takes.
//!
//! Each takes `&WebApp`, so a caller mutates inside a short `borrow_mut()` and
//! pushes after it ends.

use super::{
    Ctx,
    settings::{lighting_options, material_options},
    state::{SolveState, WebApp},
};
use crate::{AppModel, AppWindow};
use slint::{ComponentHandle, ModelRc, SharedString, TimerMode, VecModel};
use std::{rc::Rc, time::Duration};

/// How long an info/success message stays up; warnings and errors stay until
/// dismissed (the desktop's toast rule: they report something to act on).
const TOAST_AUTO_DISMISS: Duration = Duration::from_secs(5);

/// The kind of a [`show_message`] message, which colors it and decides whether
/// it auto-dismisses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// Neutral progress or information.
    Info,
    /// Something finished as asked.
    Success,
    /// Finished, but with something the user should look at.
    Warning,
    /// Did not happen.
    Error,
}

impl MessageKind {
    /// The `AppModel.message-kind` string.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

fn string_model(items: Vec<String>) -> ModelRc<SharedString> {
    let items: Vec<SharedString> = items.into_iter().map(SharedString::from).collect();
    Rc::new(VecModel::from(items)).into()
}

/// Everything: settings, design, solve status.
pub fn push_all(ui: &AppWindow, app: &WebApp) {
    push_settings(ui, app);
    push_design(ui, app);
    push_solve_status(ui, app);
}

/// Material and lighting combos (options and selection), the view tab, the camera
/// and the HDR name.
pub fn push_settings(ui: &AppWindow, app: &WebApp) {
    let model = ui.global::<AppModel>();
    let materials = material_options(&app.custom_materials);
    let material_index = materials
        .iter()
        .position(|name| name.eq_ignore_ascii_case(&app.settings.material))
        .unwrap_or(0);
    model.set_material_options(string_model(materials));
    model.set_material_index(material_index as i32);
    model.set_lighting_options(string_model(lighting_options()));
    model.set_lighting_index(app.settings.lighting_preset().index());
    model.set_view_tab(app.settings.view_tab);
    model.set_yaw(app.view.yaw);
    model.set_pitch(app.view.pitch);
    model.set_distance(app.view.distance);
    model.set_hdr_name(
        app.hdr
            .as_ref()
            .map_or_else(SharedString::new, |hdr| hdr.name.as_str().into()),
    );
}

/// The design summary: presence, name, source, dirty marker, tier count, undo/redo.
pub fn push_design(ui: &AppWindow, app: &WebApp) {
    let model = ui.global::<AppModel>();
    let Some(design) = &app.design else {
        model.set_has_design(false);
        model.set_design_name(SharedString::new());
        model.set_source_kind(SharedString::new());
        model.set_design_info(SharedString::new());
        model.set_is_dirty(false);
        model.set_tier_count(0);
        model.set_can_undo(false);
        model.set_can_redo(false);
        return;
    };
    model.set_has_design(true);
    model.set_design_name(design.display_name().into());
    model.set_source_kind(design.source.label().into());
    model.set_design_info(design.info_text().into());
    model.set_is_dirty(design.session.is_dirty());
    model.set_tier_count(design.session.design.tiers.len() as i32);
    model.set_can_undo(design.session.history.can_undo());
    model.set_can_redo(design.session.history.can_redo());
}

/// The solve status line (and whether it describes a problem).
pub fn push_solve_status(ui: &AppWindow, app: &WebApp) {
    let (text, problem) = solve_status_text(app);
    let model = ui.global::<AppModel>();
    model.set_solve_status(text.into());
    model.set_solve_problem(problem);
}

/// [`push_solve_status`]'s text: a tier-less design needs no solve; a solve for
/// an older generation is "not solved" again.
fn solve_status_text(app: &WebApp) -> (String, bool) {
    let Some(design) = &app.design else {
        return (String::new(), false);
    };
    if design.session.design.tiers.is_empty() {
        return ("Preform only -- add tiers.".to_string(), false);
    }
    let current = design.session.current_generation();
    match &app.solve {
        SolveState::Solved {
            generation,
            status,
            problem,
            took,
            ..
        } if *generation == current => (
            format!("{status} (solved in {:.2} s)", took.as_secs_f32()),
            *problem,
        ),
        SolveState::Failed {
            generation,
            message,
        } if *generation == current => (message.clone(), true),
        _ => (
            "Not solved yet -- saves and downloads solve first.".to_string(),
            false,
        ),
    }
}

/// Shows `text` in the status strip and as a toast. Info and success toasts hide
/// themselves after a few seconds; warnings and errors stay until dismissed. Also
/// written to the console, so a message is never lost with its toast.
pub fn show_message(ctx: &Ctx, kind: MessageKind, text: &str) {
    match kind {
        MessageKind::Error | MessageKind::Warning => super::diagnostics::console_warn(text),
        MessageKind::Info | MessageKind::Success => super::diagnostics::console_note(text),
    }
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<AppModel>();
    model.set_message(text.into());
    model.set_message_kind(kind.as_str().into());
    model.set_toast_visible(true);
    if matches!(kind, MessageKind::Info | MessageKind::Success) {
        let weak = ctx.ui.clone();
        ctx.timers
            .toast
            .start(TimerMode::SingleShot, TOAST_AUTO_DISMISS, move || {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<AppModel>().set_toast_visible(false);
                }
            });
    } else {
        ctx.timers.toast.stop();
    }
}
