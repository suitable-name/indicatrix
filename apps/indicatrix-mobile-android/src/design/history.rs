//! Detached history previews. Neither selecting a row nor closing the screen edits.

use std::{cell::RefCell, rc::Rc};

#[path = "history_compare.rs"]
mod comparison;
use indicatrix_mobile_core::{
    operations::{InputFingerprint, OperationIdentity, OperationPhase},
    tasks::BackgroundQueue,
};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::Shared;
use crate::{AppWindow, HistoryModel, push::toast};

struct Runtime {
    shared: Shared,
    queue: BackgroundQueue,
    preview: Option<OperationIdentity>,
}

thread_local! { static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) }; }

pub fn wire(window: &AppWindow, shared: &Shared) {
    RUNTIME.with(|runtime| {
        *runtime.borrow_mut() = Some(Runtime {
            shared: Rc::clone(shared),
            queue: BackgroundQueue::new("history-preview"),
            preview: None,
        });
    });
    let (weak, state) = (window.as_weak(), Rc::clone(shared));
    window.global::<HistoryModel>().on_show(move || {
        if let Some(window) = weak.upgrade() {
            push(&window, &state.borrow());
            let model = window.global::<HistoryModel>();
            model.set_selected(-1);
            model.set_busy(false);
            model.set_comparison("Select a step to compare it with the current design. Mast differences require a solved snapshot.".into());
            model.set_open(true);
        }
    });
    let weak = window.as_weak();
    window.global::<HistoryModel>().on_preview(move |position| {
        if let (Some(window), Ok(position)) = (weak.upgrade(), usize::try_from(position)) {
            preview(&window, position);
        }
    });
    let (weak, state) = (window.as_weak(), Rc::clone(shared));
    window.global::<HistoryModel>().on_jump(move || {
        let Some(window) = weak.upgrade() else { return };
        let Some(identity) = RUNTIME.with(|runtime| {
            runtime
                .borrow()
                .as_ref()
                .and_then(|runtime| runtime.preview)
        }) else {
            return;
        };
        let Ok(position) = usize::try_from(window.global::<HistoryModel>().get_selected()) else {
            return;
        };
        let result = {
            let mut app = state.borrow_mut();
            if app.operations.can_apply(
                identity,
                app.session.current_generation(),
                fingerprint(position),
            ) {
                indicatrix_mobile_core::history::jump_to(&mut app, position)
            } else {
                Err("This preview is stale. Select the history step again.".to_owned())
            }
        };
        if let Err(error) = result {
            toast::show(&window, &error);
        }
        super::after_edit(&window, &state);
        window.global::<HistoryModel>().set_open(false);
    });
    let (weak, state) = (window.as_weak(), Rc::clone(shared));
    window.global::<HistoryModel>().on_close(move || {
        let mut app = state.borrow_mut();
        if matches!(
            app.operations.phase(),
            Some(OperationPhase::Preparing | OperationPhase::Running)
        ) {
            app.operations.cancel();
        }
        if let Some(window) = weak.upgrade() {
            window.global::<HistoryModel>().set_open(false);
            window.global::<HistoryModel>().set_busy(false);
        }
    });
}

pub fn push(window: &AppWindow, state: &indicatrix_mobile_core::app_state::AppState) {
    let entries = std::iter::once("0 · Initial state".into())
        .chain(
            state
                .session
                .history_entries()
                .into_iter()
                .map(|entry| format!("{} · {}", entry.position, entry.label).into()),
        )
        .collect::<Vec<slint::SharedString>>();
    let model = window.global::<HistoryModel>();
    model.set_entries(ModelRc::new(VecModel::from(entries)));
    model.set_current(i32::try_from(state.session.history_position()).unwrap_or(i32::MAX));
    if matches!(
        state.operations.phase(),
        Some(OperationPhase::Stale | OperationPhase::Cancelling)
    ) {
        model.set_busy(false);
    }
}

fn fingerprint(position: usize) -> InputFingerprint {
    InputFingerprint::from_parts([
        b"history-preview-v1".as_slice(),
        position.to_le_bytes().as_slice(),
    ])
}

/// Save the state the user actually previewed; edits cannot repurpose a row number.
pub fn selected_design(
    app: &indicatrix_mobile_core::app_state::AppState,
    position: usize,
) -> Result<indicatrix_cut_core::Design, String> {
    let identity = RUNTIME
        .with(|runtime| {
            runtime
                .borrow()
                .as_ref()
                .and_then(|runtime| runtime.preview)
        })
        .ok_or_else(|| "Preview this historical step first.".to_owned())?;
    if !app.operations.can_apply(
        identity,
        app.session.current_generation(),
        fingerprint(position),
    ) {
        return Err(
            "The historical preview is stale. Preview the step again before saving.".into(),
        );
    }
    app.session
        .design_at(position)
        .map_err(|error| error.to_string())
}

fn preview(window: &AppWindow, position: usize) {
    RUNTIME.with(|runtime| {
        let mut guard = runtime.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        let (snapshot, current, token, policy) = {
            let mut app = runtime.shared.borrow_mut();
            // Opening Variants must not assign a family and invalidate this preview.
            indicatrix_mobile_core::document::ensure_document_id(&mut app);
            let generation = app.session.current_generation();
            let token = app.operations.begin(generation, fingerprint(position));
            app.operations.publish(
                token.identity,
                generation,
                token.identity.inputs,
                OperationPhase::Running,
            );
            (
                app.session.history_snapshot(),
                app.session.design.clone(),
                token,
                app.render.compute_policy(),
            )
        };
        runtime.preview = Some(token.identity);
        window.global::<HistoryModel>().set_busy(true);
        window
            .global::<HistoryModel>()
            .set_selected(i32::try_from(position).unwrap_or(-1));
        let weak = window.as_weak();
        runtime.queue.submit(move || {
            if token.is_cancelled() {
                return;
            }
            let Some(permit) = policy.acquire(&|| token.is_cancelled()) else {
                return;
            };
            let stop = || token.is_cancelled() || policy.paused();
            let result = snapshot
                .design_at(position)
                .map(|design| comparison::compare(design, &current, &stop))
                .map_err(|error| error.to_string());
            drop(permit);
            if token.is_cancelled() {
                return;
            }
            let _queued = weak.upgrade_in_event_loop(move |window| {
                RUNTIME.with(|runtime| {
                    let guard = runtime.borrow();
                    let Some(runtime) = guard.as_ref() else {
                        return;
                    };
                    let mut app = runtime.shared.borrow_mut();
                    let generation = app.session.current_generation();
                    let phase = match &result {
                        Ok(_) => OperationPhase::Completed,
                        Err(error) => OperationPhase::Failed(error.clone()),
                    };
                    if !app.operations.publish(
                        token.identity,
                        generation,
                        fingerprint(position),
                        phase,
                    ) {
                        return;
                    }
                    let model = window.global::<HistoryModel>();
                    model.set_busy(false);
                    model.set_comparison(result.unwrap_or_else(|error| error).into());
                });
            });
        });
    });
}

pub fn shutdown() {
    RUNTIME.with(|runtime| drop(runtime.borrow_mut().take()));
}
