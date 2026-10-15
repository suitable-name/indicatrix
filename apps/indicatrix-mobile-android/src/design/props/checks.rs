//! The Checks tab: the verdict, the findings with their fixes, the proportions and the
//! preform fields.
//!
//! A fix is planned on a worker thread (planning solves candidate designs) and applied on
//! the UI thread as one undoable edit, unless the design moved on meanwhile.

use indicatrix_mobile_core::{
    app_state::AppState,
    design_props::{
        ChecksView, PlannedFix, ProportionRow, WarningRow, apply_planned_fix, checks, fix_request,
    },
};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::{HANDLE, Pushed, bind1, string_model};
use crate::{
    AppWindow, ProportionItem, PropsModel, WarningItem,
    design::{Shared, after_edit, refresh},
    push::toast,
};

/// Connects the Checks tab's callbacks.
pub(super) fn wire(window: &AppWindow, state: &Shared) {
    let props = window.global::<PropsModel>();
    props.on_fix_warning(bind1(window, state, fix_warning));
    props.on_show_warning(bind1(window, state, show_warning));
}

/// The Fix button of finding `index` was pressed: plan on a worker, then apply.
fn fix_warning(window: &AppWindow, state: &Shared, index: i32) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    let request = fix_request(&state.borrow(), index);
    match request {
        Err(message) => toast::show(window, &message),
        Ok(request) => {
            let weak = window.as_weak();
            std::thread::spawn(move || {
                let planned = request.plan();
                let queued = weak.upgrade_in_event_loop(move |window| finish_fix(&window, planned));
                if queued.is_err() {
                    tracing::warn!("a planned fix could not be handed back to the UI thread");
                }
            });
        }
    }
}

/// The planner's answer, back on the UI thread: apply it when it still fits the design.
fn finish_fix(window: &AppWindow, planned: Result<PlannedFix, String>) {
    let Some(state) = HANDLE.with(|handle| handle.borrow().clone()) else {
        return;
    };
    let applied = planned.and_then(|plan| apply_planned_fix(&mut state.borrow_mut(), plan));
    match applied {
        Ok(applied) => {
            toast::show(window, &format!("{} Undo puts it back.", applied.message));
            after_edit(window, &state);
        }
        Err(message) => toast::show(window, &message),
    }
}

/// "Show" on finding `index`: select its tier.
fn show_warning(window: &AppWindow, state: &Shared, index: i32) {
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    let tier = {
        let app = state.borrow();
        checks(&app)
            .warnings
            .get(index)
            .and_then(|warning| warning.tier)
    };
    if let Some(tier) = tier {
        state.borrow_mut().selected_tier = Some(tier);
        refresh(window, state);
    }
}

/// The Slint row of a finding.
fn warning_item(row: &WarningRow) -> WarningItem {
    WarningItem {
        text: row.text.as_str().into(),
        level: row.level,
        tier: row
            .tier
            .and_then(|tier| i32::try_from(tier).ok())
            .unwrap_or(-1),
        fix_label: row.fix_label.clone().unwrap_or_default().into(),
        fix_hint: row.fix_hint.clone().unwrap_or_default().into(),
        confirm: row.confirm.clone().unwrap_or_default().into(),
    }
}

/// The Slint row of a proportion.
fn proportion_item(row: &ProportionRow) -> ProportionItem {
    ProportionItem {
        label: row.label.as_str().into(),
        value: row.value.as_str().into(),
        level: row.level,
        reason: row.reason.as_str().into(),
    }
}

/// Pushes the Checks tab. The verdict (the Design page's peek bar reads it) is written on
/// every push; the lists and the preform fields only when they changed.
pub(super) fn push(props: &PropsModel, state: &AppState, pushed: &mut Pushed) {
    let view = checks(state);
    props.set_verdict_text(view.verdict_text.as_str().into());
    props.set_verdict_level(view.verdict_level);
    if pushed.checks.as_ref() == Some(&view) {
        return;
    }
    push_changes(props, &view, pushed.checks.as_ref());
    pushed.checks = Some(view);
}

/// Writes what differs between `view` and the `was` pushed before it.
fn push_changes(props: &PropsModel, view: &ChecksView, was: Option<&ChecksView>) {
    props.set_checks_ready(view.ready);
    props.set_checks_note(view.note.as_str().into());
    if was.is_none_or(|old| old.warnings != view.warnings) {
        let items: Vec<WarningItem> = view.warnings.iter().map(warning_item).collect();
        props.set_warnings(ModelRc::new(VecModel::from(items)));
    }
    if was.is_none_or(|old| old.proportions != view.proportions) {
        let items: Vec<ProportionItem> = view.proportions.iter().map(proportion_item).collect();
        props.set_proportions(ModelRc::new(VecModel::from(items)));
    }
    if was.is_none_or(|old| old.yield_lines != view.yield_lines) {
        props.set_yield_lines(string_model(view.yield_lines.clone()));
    }

    let form = &view.preform;
    let old = was.map(|old| &old.preform);
    props.set_preform_shape(form.shape_label.as_str().into());
    props.set_preform_note(form.source_note.as_str().into());
    props.set_preform_editable(form.editable);
    props.set_preform_half_width_mm(form.half_width_mm.as_str().into());
    props.set_preform_depth_mm(form.depth_mm.as_str().into());
    if old.is_none_or(|old| old.half_width != form.half_width) {
        props.set_preform_half_width(form.half_width.as_str().into());
    }
    if old.is_none_or(|old| old.length_over_width != form.length_over_width) {
        props.set_preform_length_width(form.length_over_width.as_str().into());
    }
    if old.is_none_or(|old| old.depth != form.depth) {
        props.set_preform_depth(form.depth.as_str().into());
    }
    if old.is_none_or(|old| old.girdle_mm != form.girdle_mm) {
        props.set_preform_girdle_mm(form.girdle_mm.as_str().into());
    }
    if old.is_none_or(|old| old.specific_gravity != form.specific_gravity) {
        props.set_preform_sg(form.specific_gravity.as_str().into());
    }
}
