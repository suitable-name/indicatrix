//! The Checks tab's Metrics section and the Tilt Curves sheet.
//!
//! The measuring happens on the core's analysis thread (`indicatrix_mobile_core::analysis`);
//! this module requests it, shows the newest results, and pushes progress. The analysis
//! thread calls a notifier when something changed; the notifier hops to the UI thread and
//! re-pushes this tab only (not the whole Design page).

use std::sync::Arc;

use indicatrix_mobile_core::{
    analysis::{
        self, AnalysisStatus, MetricsView, Notifier, TiltReading, metrics_view, request_metrics,
        request_tilt, tilt_view,
    },
    app_state::AppState,
};
use slint::{ComponentHandle, ModelRc, VecModel};

use super::{HANDLE, Pushed, bind0, bind1, string_model};
use crate::{AppWindow, PropsModel, design::Shared, push::toast};

/// What the last push wrote for the analysis surfaces.
pub(super) struct AnalysisPushed {
    metrics: Option<MetricsView>,
    /// The tilt curves on screen: the reading's address and the axis drawn.
    tilt: Option<(usize, usize)>,
}

impl AnalysisPushed {
    /// Nothing pushed yet.
    pub(super) const EMPTY: Self = Self {
        metrics: None,
        tilt: None,
    };
}

/// Connects the Metrics section's and the Tilt sheet's callbacks and the analysis notifier.
pub(super) fn wire(window: &AppWindow, state: &Shared) {
    let props = window.global::<PropsModel>();
    props.set_tilt_axis_names(string_model(TiltReading::axis_names()));
    props.on_metrics_compute(bind0(window, state, compute_metrics));
    props.on_tilt_opened(bind0(window, state, tilt_opened));
    props.on_tilt_run(bind0(window, state, run_tilt));
    props.on_tilt_cancel(bind0(window, state, cancel_tilt));
    props.on_tilt_axis_picked(bind1(window, state, axis_picked));
    props.on_tilt_copy(bind0(window, state, copy_data));

    let weak = window.as_weak();
    let notifier: Notifier = Arc::new(move || {
        let queued = weak.upgrade_in_event_loop(|window| {
            if let Some(state) = HANDLE.with(|handle| handle.borrow().clone()) {
                super::push(&window, &state.borrow());
            }
        });
        if queued.is_err() {
            tracing::debug!("the window closed before an analysis result could be shown");
        }
    });
    state.borrow().analysis.set_notifier(Some(notifier));
}

/// Removes the notifier (see `props::forget`).
pub(super) fn unwire(state: &Shared) {
    if let Ok(app) = state.try_borrow() {
        app.analysis.set_notifier(None);
    }
}

/// "Compute" on the Metrics section.
fn compute_metrics(window: &AppWindow, state: &Shared) {
    let result = request_metrics(&mut state.borrow_mut());
    if let Err(message) = result {
        toast::show(window, &message);
    }
    super::push(window, &state.borrow());
}

/// The Tilt sheet opened: sweep unless the curves already describe this stone.
fn tilt_opened(window: &AppWindow, state: &Shared) {
    let status = tilt_view(&state.borrow()).status;
    if matches!(
        status,
        AnalysisStatus::NotComputed | AnalysisStatus::Stale | AnalysisStatus::Failed
    ) {
        run_tilt(window, state);
    } else {
        super::push(window, &state.borrow());
    }
}

/// "Run" on the Tilt sheet.
fn run_tilt(window: &AppWindow, state: &Shared) {
    let result = request_tilt(&mut state.borrow_mut());
    if let Err(message) = result {
        toast::show(window, &message);
    }
    super::push(window, &state.borrow());
}

/// "Cancel" on the Tilt sheet: the curves already on screen stay.
fn cancel_tilt(window: &AppWindow, state: &Shared) {
    state.borrow().analysis.cancel_tilt();
    super::push(window, &state.borrow());
}

/// An axis chip was picked: redraw the curves of that azimuth.
fn axis_picked(window: &AppWindow, state: &Shared, _index: i32) {
    // The sheet already wrote the index into `tilt-axis`; the push reads it back.
    super::push(window, &state.borrow());
}

/// "Copy Data": the whole sweep as a tab-separated table.
fn copy_data(window: &AppWindow, state: &Shared) {
    let app = state.borrow();
    let Some(curves) = app.analysis.latest_tilt() else {
        toast::show(window, "Run the sweep first.");
        return;
    };
    match app.platform().set_clipboard_text(&curves.to_tsv()) {
        Ok(()) => toast::show(window, "Tilt data copied."),
        Err(error) => toast::show(window, &error.message),
    }
}

/// Pushes the Metrics section and the Tilt sheet. Also stops a job whose inputs changed
/// under it.
pub(super) fn push(props: &PropsModel, state: &AppState, pushed: &mut Pushed) {
    analysis::supersede_stale(state);
    push_metrics(props, state, &mut pushed.analysis);
    push_tilt(props, state, &mut pushed.analysis);
}

/// The Metrics section.
fn push_metrics(props: &PropsModel, state: &AppState, pushed: &mut AnalysisPushed) {
    let view = metrics_view(state);
    if pushed.metrics.as_ref() == Some(&view) {
        return;
    }
    props.set_metrics_state(view.status.code());
    props.set_metrics_note(view.message.as_str().into());
    props.set_metric_labels(string_model(
        view.rows.iter().map(|row| row.label.to_owned()),
    ));
    props.set_metric_values(string_model(view.rows.iter().map(|row| row.value.clone())));
    props.set_metric_meanings(string_model(
        view.rows.iter().map(|row| row.meaning.to_owned()),
    ));
    pushed.metrics = Some(view);
}

/// The Tilt sheet: status and progress on every push, the curves only when they or the
/// chosen axis changed.
fn push_tilt(props: &PropsModel, state: &AppState, pushed: &mut AnalysisPushed) {
    let view = tilt_view(state);
    props.set_tilt_state(view.status.code());
    props.set_tilt_note(view.message.as_str().into());
    props.set_tilt_progress(view.progress.unwrap_or(-1.0));
    props.set_tilt_stage(view.stage.as_str().into());

    let axis = usize::try_from(props.get_tilt_axis()).unwrap_or(0);
    let key = view
        .curves
        .as_ref()
        .map(|curves| (Arc::as_ptr(curves) as usize, axis));
    if pushed.tilt == key {
        return;
    }
    pushed.tilt = key;
    let drawn = view.curves.as_ref().and_then(|curves| {
        let paths = curves.paths(axis)?;
        let data = curves.axis(axis)?;
        Some((paths, data.clone()))
    });
    if let Some(([brilliance, extinction, windowing], data)) = drawn {
        props.set_tilt_brilliance_path(brilliance.as_str().into());
        props.set_tilt_extinction_path(extinction.as_str().into());
        props.set_tilt_windowing_path(windowing.as_str().into());
        props.set_tilt_brilliance(ModelRc::new(VecModel::from(data.brilliance)));
        props.set_tilt_extinction(ModelRc::new(VecModel::from(data.extinction)));
        props.set_tilt_windowing(ModelRc::new(VecModel::from(data.windowing)));
        props.set_tilt_has_curves(true);
    } else {
        props.set_tilt_brilliance_path("".into());
        props.set_tilt_extinction_path("".into());
        props.set_tilt_windowing_path("".into());
        props.set_tilt_brilliance(ModelRc::default());
        props.set_tilt_extinction(ModelRc::default());
        props.set_tilt_windowing(ModelRc::default());
        props.set_tilt_has_curves(false);
    }
}
