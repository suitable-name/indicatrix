//! The export sheet: choose size, samples and color space, render the picture on a worker
//! thread with progress and Cancel, then share it (and offer Save).
//!
//! The options, the caps, the estimate and the file name come from the core
//! (`render_view`); the picture itself from `render::export_job` and `ExportJob::run`.
//! The job owns a snapshot of the scene, so the design may change while it renders.
//!
//! Threading: the export thread reports through `slint::Weak::upgrade_in_event_loop` (at
//! most once per percent) and never touches the app state. Closing the sheet cancels a
//! running job; [`shutdown`] cancels and joins the thread.

use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread::JoinHandle,
};

use indicatrix_mobile_core::{
    platform::{OutgoingFile, SaveOutcome, mime},
    render::{self, ExportRequest},
    tasks::join_briefly,
    viewport::render_view::{
        ExportEstimate, byte_size_text, color_space_from_index, estimate_text, export_file_name,
        export_request, size_options, spp_options,
    },
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::runtime;
use crate::{AppWindow, RenderModel, design::Shared, push::toast};

/// What the sheet remembers between its callbacks.
#[derive(Default)]
struct ExportState {
    /// The measured time of the sheet's probe job, to scale to other options.
    estimate: Option<ExportEstimate>,
    /// The cancel flag of the running job.
    cancel: Option<Arc<AtomicBool>>,
    /// The running job's thread.
    handle: Option<JoinHandle<()>>,
    /// The finished picture, for Share and Save.
    file: Option<OutgoingFile>,
}

thread_local! {
    static EXPORT: RefCell<ExportState> = RefCell::new(ExportState::default());
}

/// Connects the sheet's callbacks.
pub(super) fn wire(window: &AppWindow, state: &Shared) {
    let model = window.global::<RenderModel>();

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_export_options_changed(move || {
        if let Some(window) = weak.upgrade() {
            show_estimate(&window, &shared);
        }
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_export_render(move || {
        if let Some(window) = weak.upgrade() {
            render_now(&window, &shared);
        }
    });

    model.on_export_cancel(cancel);

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_export_share(move || {
        let Some(window) = weak.upgrade() else { return };
        let file = EXPORT.with(|export| export.borrow().file.clone());
        if let Some(file) = file {
            share_file(&window, &shared, file);
        }
    });

    let (weak, shared) = (window.as_weak(), state.clone());
    model.on_export_save(move || {
        let Some(window) = weak.upgrade() else { return };
        let file = EXPORT.with(|export| export.borrow().file.clone());
        if let Some(file) = file {
            save_file(&window, &shared, file);
        }
    });

    let weak = window.as_weak();
    model.on_export_closed(move || {
        if let Some(window) = weak.upgrade() {
            close(&window);
        }
    });
}

/// Cancels a running job and joins its thread. Call on the UI thread when the event loop
/// has ended.
pub(super) fn shutdown() {
    let (handle, cancel) = EXPORT.with(|export| {
        let mut export = export.borrow_mut();
        (export.handle.take(), export.cancel.take())
    });
    if let Some(cancel) = cancel {
        cancel.store(true, Ordering::Relaxed);
    }
    if let Some(handle) = handle {
        join_briefly(handle, "render-export");
    }
}

/// The request the sheet's three controls describe.
fn current_request(window: &AppWindow, shared: &Shared) -> ExportRequest {
    let model = window.global::<RenderModel>();
    let size = usize::try_from(model.get_export_size()).unwrap_or(0);
    let spp = usize::try_from(model.get_export_spp()).unwrap_or(0);
    let color = color_space_from_index(model.get_export_color());
    export_request(&shared.borrow().settings.render, size, spp, color)
}

/// Shows the time the chosen options will take, scaled from the probe job.
fn show_estimate(window: &AppWindow, shared: &Shared) {
    let model = window.global::<RenderModel>();
    if !model.get_export_ready() {
        model.set_export_estimate(SharedString::new());
        return;
    }
    let request = current_request(window, shared);
    let seconds = EXPORT.with(|export| {
        export
            .borrow()
            .estimate
            .and_then(|estimate| estimate.for_request(&request))
    });
    model.set_export_estimate(estimate_text(seconds).into());
}

/// Opens the sheet with the options the settings' caps allow (the largest size, 128
/// samples, sRGB). A probe job is built once to learn whether the scene can be exported
/// and how long it takes; the sheet shows the reason when it cannot.
pub(super) fn open(window: &AppWindow, shared: &Shared) {
    let model = window.global::<RenderModel>();
    let (sizes, spps, size_index, spp_index, probe) = {
        let app = shared.borrow();
        let settings = &app.settings.render;
        let sizes = size_options(settings.export_cap_px);
        let spps = spp_options(settings.export_cap_spp);
        let size_index = sizes.len().saturating_sub(1);
        let spp_index = 1_usize.min(spps.len().saturating_sub(1));
        let request = export_request(settings, size_index, spp_index, color_space_from_index(0));
        let probe = render::export_job(&app, &request)
            .map(|job| ExportEstimate::new(&request, job.estimate_seconds()));
        (sizes, spps, size_index, spp_index, probe)
    };

    model.set_export_sizes(labels(&sizes, " px"));
    model.set_export_samples(labels(&spps, ""));
    model.set_export_size(i32::try_from(size_index).unwrap_or(0));
    model.set_export_spp(i32::try_from(spp_index).unwrap_or(0));
    model.set_export_color(0);
    model.set_export_running(false);
    model.set_export_done(false);
    model.set_export_progress(0.0);
    model.set_export_result(SharedString::new());
    EXPORT.with(|export| {
        let mut export = export.borrow_mut();
        export.file = None;
        export.estimate = probe.as_ref().ok().copied();
    });
    match probe {
        Ok(_) => {
            model.set_export_ready(true);
            model.set_export_problem(SharedString::new());
            show_estimate(window, shared);
        }
        Err(message) => {
            model.set_export_ready(false);
            model.set_export_problem(message.into());
            model.set_export_estimate(SharedString::new());
        }
    }
    model.set_export_open(true);
}

/// The option labels as a Slint model ("512 px").
fn labels(values: &[u32], suffix: &str) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        values
            .iter()
            .map(|value| SharedString::from(format!("{value}{suffix}")))
            .collect::<Vec<_>>(),
    ))
}

/// Closes the sheet; a running job is cancelled.
fn close(window: &AppWindow) {
    cancel();
    let model = window.global::<RenderModel>();
    model.set_export_open(false);
    model.set_export_done(false);
    EXPORT.with(|export| export.borrow_mut().file = None);
}

/// Asks the running job to stop (the job's thread answers with a cancelled result).
fn cancel() {
    EXPORT.with(|export| {
        if let Some(cancel) = &export.borrow().cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    });
}

/// Builds the job for the chosen options and renders it on a worker thread.
fn render_now(window: &AppWindow, shared: &Shared) {
    let model = window.global::<RenderModel>();
    if model.get_export_running() {
        return;
    }
    let request = current_request(window, shared);
    let job = {
        let app = shared.borrow();
        render::export_job(&app, &request)
    };
    let job = match job {
        Ok(job) => job,
        Err(message) => {
            model.set_export_ready(false);
            model.set_export_problem(message.into());
            return;
        }
    };

    let cancel = Arc::new(AtomicBool::new(false));
    let thread_cancel = Arc::clone(&cancel);
    let (done_weak, progress_weak) = (window.as_weak(), window.as_weak());
    let spawned = std::thread::Builder::new()
        .name("render-export".to_owned())
        .spawn(move || {
            let last_percent = AtomicU32::new(u32::MAX);
            let report = |fraction: f32| {
                let percent = percent_of(fraction);
                if last_percent.swap(percent, Ordering::Relaxed) != percent {
                    let _queued = progress_weak.upgrade_in_event_loop(move |window| {
                        window
                            .global::<RenderModel>()
                            .set_export_progress(percent_to_fraction(percent));
                    });
                }
            };
            let result = catch_unwind(AssertUnwindSafe(|| job.run(&report, &thread_cancel)))
                .unwrap_or_else(|_| Err("The export stopped unexpectedly.".to_owned()));
            let cancelled = thread_cancel.load(Ordering::Relaxed);
            let _queued =
                done_weak.upgrade_in_event_loop(move |window| finished(&window, result, cancelled));
        });
    match spawned {
        Ok(handle) => {
            EXPORT.with(|export| {
                let mut export = export.borrow_mut();
                export.cancel = Some(cancel);
                export.handle = Some(handle);
                export.file = None;
            });
            model.set_export_running(true);
            model.set_export_done(false);
            model.set_export_progress(0.0);
            model.set_export_problem(SharedString::new());
        }
        Err(error) => {
            tracing::error!(%error, "the export thread could not be started");
            model.set_export_problem("The export could not be started.".into());
        }
    }
}

/// A fraction as a whole percent, `0..=100`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the fraction is clamped to 0 to 1 first"
)]
fn percent_of(fraction: f32) -> u32 {
    (fraction.clamp(0.0, 1.0) * 100.0).round() as u32
}

/// A whole percent as a fraction.
#[allow(clippy::cast_precision_loss, reason = "the percent is at most 100")]
fn percent_to_fraction(percent: u32) -> f32 {
    percent as f32 / 100.0
}

/// The job ended (on the UI thread): show the result, or why there is none, and offer the
/// picture to other apps.
fn finished(window: &AppWindow, result: Result<Vec<u8>, String>, cancelled: bool) {
    let model = window.global::<RenderModel>();
    model.set_export_running(false);
    EXPORT.with(|export| {
        let mut export = export.borrow_mut();
        export.cancel = None;
        // The thread is done but for this hop; detach it.
        drop(export.handle.take());
    });
    if cancelled {
        model.set_export_problem("Cancelled.".into());
        return;
    }
    let bytes = match result {
        Ok(bytes) => bytes,
        Err(message) => {
            model.set_export_problem(message.as_str().into());
            toast::show(window, &message);
            return;
        }
    };
    let Some((_, shared)) = runtime::context() else {
        return;
    };
    let title = shared.borrow().design_title();
    let file = OutgoingFile {
        suggested_name: export_file_name(&title),
        mime: mime::PNG.to_owned(),
        bytes,
    };
    model.set_export_result(
        format!(
            "Picture ready: {} ({}).",
            file.suggested_name,
            byte_size_text(file.bytes.len())
        )
        .into(),
    );
    model.set_export_problem(SharedString::new());
    model.set_export_done(true);
    EXPORT.with(|export| export.borrow_mut().file = Some(file.clone()));
    share_file(window, &shared, file);
}

/// Offers `file` to other apps.
fn share_file(window: &AppWindow, shared: &Shared, file: OutgoingFile) {
    // Clone the Arc first: a platform may answer on the calling thread.
    let platform = Arc::clone(shared.borrow().platform());
    if let Err(error) = platform.share(file) {
        toast::show(window, &error.message);
    }
}

/// Asks where to save `file`.
fn save_file(window: &AppWindow, shared: &Shared, file: OutgoingFile) {
    let platform = Arc::clone(shared.borrow().platform());
    let weak = window.as_weak();
    platform.create_document(
        file,
        Box::new(move |result| {
            let _queued = weak.upgrade_in_event_loop(move |window| match result {
                Ok(SaveOutcome::Saved { display_name }) => {
                    toast::show(&window, &format!("Saved {display_name}"));
                }
                Ok(SaveOutcome::Cancelled) => {}
                Err(error) => toast::show(&window, &error.message),
            });
        }),
    );
}
