//! Remote setup commands use a separate worker and reject replaced connection results.
//!
//! Forget Server also lives here: a confirmation dialog (id [`DIALOG_FORGET`], answered
//! in [`handle_dialog_result`]), then the worker erases the stored credentials and the UI
//! thread clears the saved server and leaves the Remote render backend.
use crate::{AppWindow, JobRow, RemoteModel, Shell, push::toast};
use indicatrix_mobile_core::{
    app_state::AppState,
    remote::{self, ConnectionReport},
    settings::RemoteTransfer,
    tasks::BackgroundQueue,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use zeroize::Zeroizing;

/// The dialog id of "Forget Server?" (ids 4000-4099 belong to this page; `design::wire`
/// routes the answer to [`handle_dialog_result`]).
pub const DIALOG_FORGET: i32 = 4001;

/// The question the dialog asks. It names what is deleted, that it cannot be undone and
/// what stays on the server (the phone cannot revoke its own certificate there).
const FORGET_BODY: &str = concat!(
    "This deletes the pairing credentials on this phone, including the Android Keystore ",
    "key that protects them, and clears the saved server name and address. It cannot be ",
    "undone: to use the server again you must pair this phone with a new one-time token. ",
    "If Remote rendering is selected, it switches back to Automatic. The server keeps its ",
    "own list of paired phones and any server jobs until its operator removes them.",
);

struct Runtime {
    window: slint::Weak<AppWindow>,
    state: Rc<RefCell<AppState>>,
    queue: BackgroundQueue,
    next: u64,
    active: Option<(u64, Arc<AtomicBool>)>,
    installing: bool,
    /// The worker is erasing the stored credentials; setup requests wait for it.
    forgetting: bool,
}
thread_local! { static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) }; }

pub fn wire(window: &AppWindow, state: &Rc<RefCell<AppState>>) {
    RUNTIME.with(|value| {
        *value.borrow_mut() = Some(Runtime {
            window: window.as_weak(),
            state: Rc::clone(state),
            queue: BackgroundQueue::new("remote-setup"),
            next: 0,
            active: None,
            installing: false,
            forgetting: false,
        });
    });
    let model = window.global::<RemoteModel>();
    let app = state.borrow();
    model.set_name(app.settings.remote.name.as_str().into());
    model.set_address(app.settings.remote.address.as_str().into());
    model.set_transfer(i32::from(
        app.settings.remote.transfer == RemoteTransfer::FullData,
    ));
    model.set_cadence(i32::try_from(app.settings.remote.cadence_ms).unwrap_or(500));
    model.set_allow_metered(app.settings.remote.allow_metered);
    drop(app);
    crate::remote_jobs::wire(window, state);
    let weak = window.as_weak();
    model.on_save(move || {
        if let Some(window) = weak.upgrade() {
            save(&window);
        }
    });
    let weak = window.as_weak();
    model.on_enroll(move || {
        if let Some(window) = weak.upgrade() {
            start(&window, true);
        }
    });
    let weak = window.as_weak();
    model.on_test(move || {
        if let Some(window) = weak.upgrade() {
            start(&window, false);
        }
    });
    let weak = window.as_weak();
    model.on_cancel(move || {
        interrupt();
        if let Some(window) = weak.upgrade() {
            window.global::<RemoteModel>().set_token("".into());
        }
    });
    let weak = window.as_weak();
    model.on_hidden(move || {
        interrupt();
        if let Some(window) = weak.upgrade() {
            window.global::<RemoteModel>().set_token("".into());
        }
    });
    let weak = window.as_weak();
    model.on_close(move || {
        interrupt();
        if let Some(window) = weak.upgrade() {
            window.global::<RemoteModel>().set_token("".into());
            window.global::<Shell>().set_more_page(0);
        }
    });
    let weak = window.as_weak();
    model.on_forget_server(move || {
        if let Some(window) = weak.upgrade() {
            ask_forget(&window);
        }
    });
}

/// Answers this page's dialog (Forget Server). Returns false for any other dialog id.
pub fn handle_dialog_result(window: &AppWindow, id: i32, confirmed: bool) -> bool {
    if id != DIALOG_FORGET {
        return false;
    }
    if confirmed {
        start_forget(window);
    }
    true
}

/// Asks before the pairing credentials are deleted. The answer arrives in
/// [`handle_dialog_result`].
fn ask_forget(window: &AppWindow) {
    window.global::<Shell>().invoke_show_dialog(
        DIALOG_FORGET,
        "Forget Server?".into(),
        FORGET_BODY.into(),
        "Forget Server".into(),
        "Cancel".into(),
    );
}

/// Starts erasing the credentials once the user confirmed (on the UI thread). Nothing in
/// the settings changes until the worker reports success, so a failure can be retried.
fn start_forget(window: &AppWindow) {
    let model = window.global::<RemoteModel>();
    let Some(busy) = RUNTIME.with(|value| {
        value
            .borrow()
            .as_ref()
            .map(|runtime| runtime.installing || runtime.forgetting)
    }) else {
        return;
    };
    if busy {
        model.set_status(
            "Credentials are being changed. Try Forget Server again in a moment.".into(),
        );
        return;
    }
    // A connection test, a jobs request, or a picture export or Send To Server upload that
    // talks to the server must not hold the credentials while they are erased: each is
    // cancelled (its connection ends at its next poll) and its result is dropped. A render
    // that runs on this phone is left alone; the live remote render is stopped when the
    // erase ends (`finish_forget`).
    interrupt();
    crate::design::render::cancel_remote_export();
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        runtime.forgetting = true;
        model.set_forgetting(true);
        model.set_status("Deleting the stored pairing credentials…".into());
        let platform = Arc::clone(runtime.state.borrow().platform());
        let weak = window.as_weak();
        runtime.queue.submit(move || {
            let result = remote::forget(platform.as_ref());
            let _queued = weak.upgrade_in_event_loop(move |window| finish_forget(&window, result));
        });
    });
}

/// The erase ended (on the UI thread). On success the saved server is cleared, Remote
/// rendering is left and the page is emptied; on failure everything stays as it was.
fn finish_forget(window: &AppWindow, result: Result<(), String>) {
    let model = window.global::<RemoteModel>();
    let state = RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let runtime = guard.as_mut()?;
        runtime.forgetting = false;
        Some(Rc::clone(&runtime.state))
    });
    model.set_forgetting(false);
    let Some(state) = state else {
        return;
    };
    if let Err(error) = result {
        let message = format!("{error} The server is still remembered; try Forget Server again.");
        model.set_status(message.as_str().into());
        toast::show(window, &message);
        return;
    }
    let left_remote = {
        let mut app = state.borrow_mut();
        let mut switched = false;
        app.update_settings(|settings| {
            switched = remote::clear_server_settings(settings);
        });
        if switched {
            // The same steps as picking another backend in the Render view.
            app.render.set_user_paused(false);
            app.render.stop();
        }
        switched
    };
    model.set_name("".into());
    model.set_address("".into());
    model.set_enrollment_address("".into());
    model.set_token("".into());
    model.set_identity("".into());
    crate::remote_jobs::clear();
    model.set_jobs(ModelRc::new(VecModel::from(Vec::<JobRow>::new())));
    model.set_jobs_status("No server is set up, so there are no server jobs to show.".into());
    model.set_status("Server forgotten. Enroll again with a new token to use a server.".into());
    crate::design::refresh(window, &state);
    toast::show(
        window,
        if left_remote {
            "Server forgotten. Rendering is back on Automatic."
        } else {
            "Server forgotten."
        },
    );
}

fn save(window: &AppWindow) {
    let model = window.global::<RemoteModel>();
    RUNTIME.with(|value| {
        let guard = value.borrow();
        let Some(runtime) = guard.as_ref() else {
            return;
        };
        runtime.state.borrow_mut().update_settings(|settings| {
            model
                .get_name()
                .trim()
                .clone_into(&mut settings.remote.name);
            model
                .get_address()
                .trim()
                .clone_into(&mut settings.remote.address);
            settings.remote.transfer = if model.get_transfer() == 1 {
                RemoteTransfer::FullData
            } else {
                RemoteTransfer::DisplayOnly
            };
            settings.remote.cadence_ms = u32::try_from(model.get_cadence())
                .unwrap_or(500)
                .clamp(100, 10000);
            settings.remote.allow_metered = model.get_allow_metered();
        });
        model.set_status("Connection settings saved. No design was uploaded.".into());
    });
}

enum Outcome {
    Claimed(remote::Enrollment),
    Enrolled,
    Tested(ConnectionReport),
}
fn start(window: &AppWindow, enrollment: bool) {
    save(window);
    let model = window.global::<RemoteModel>();
    let address = if enrollment {
        model.get_enrollment_address()
    } else {
        model.get_address()
    }
    .trim()
    .to_owned();
    let token = Zeroizing::new(model.get_token().to_string());
    model.set_token("".into());
    model.set_identity("".into());
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        if runtime.installing || runtime.forgetting {
            return;
        }
        if let Some((_, old)) = runtime.active.take() {
            old.store(true, Ordering::Release);
        }
        runtime.next = runtime.next.wrapping_add(1);
        let id = runtime.next;
        let cancelled = Arc::new(AtomicBool::new(false));
        runtime.active = Some((id, Arc::clone(&cancelled)));
        let platform = Arc::clone(runtime.state.borrow().platform());
        model.set_busy(true);
        model.set_status(
            if enrollment {
                "Claiming a CA-pinned enrollment token…"
            } else {
                "Authenticating and checking protocol/engine compatibility…"
            }
            .into(),
        );
        let weak = window.as_weak();
        runtime.queue.submit(move || {
            let outcome = if enrollment {
                remote::enroll(&token, &address, &cancelled).map(Outcome::Claimed)
            } else {
                remote::test_connection(platform.as_ref(), &address, &cancelled)
                    .map(Outcome::Tested)
            };
            let _queued = weak.upgrade_in_event_loop(move |window| {
                finish(&window, id, address, enrollment, outcome);
            });
        });
    });
}
fn finish(
    window: &AppWindow,
    id: u64,
    address: String,
    enrollment: bool,
    outcome: Result<Outcome, String>,
) {
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        if !runtime
            .active
            .as_ref()
            .is_some_and(|(run, cancel)| *run == id && !cancel.load(Ordering::Acquire))
        {
            return;
        }
        let model = window.global::<RemoteModel>();
        if runtime.installing {
            runtime.installing = false;
            model.set_installing(false);
        } else {
            let shell = window.global::<Shell>();
            if shell.get_destination() != 3 || shell.get_more_page() != 4 {
                runtime.active = None;
                model.set_busy(false);
                model.set_status("Connection interrupted when leaving Remote.".into());
                return;
            }
            let current = if enrollment {
                model.get_enrollment_address()
            } else {
                model.get_address()
            };
            if current.trim() != address {
                runtime.active = None;
                model.set_busy(false);
                model.set_status("Address changed; connection result discarded.".into());
                return;
            }
        }
        if let Ok(Outcome::Claimed(bundle)) = outcome {
            // UI acceptance is the commit boundary. Cancellation before this point
            // drops the claim; afterward the atomic install is allowed to finish.
            runtime.installing = true;
            model.set_installing(true);
            model.set_status("Installing encrypted credentials…".into());
            let platform = Arc::clone(runtime.state.borrow().platform());
            let weak = window.as_weak();
            runtime.queue.submit(move || {
                let result = bundle
                    .install(platform.as_ref())
                    .map(|()| Outcome::Enrolled);
                let _queued = weak.upgrade_in_event_loop(move |window| {
                    finish(&window, id, address, true, result);
                });
            });
            return;
        }
        runtime.active = None;
        model.set_busy(false);
        match outcome {
            Ok(Outcome::Claimed(_)) => unreachable!("claim handled above"),
            Ok(Outcome::Enrolled) => {
                runtime
                    .state
                    .borrow_mut()
                    .update_settings(|settings| settings.remote.enrolled = true);
                model.set_status(
                    "Credentials encrypted and installed. Test the rendering address next.".into(),
                );
            }
            Ok(Outcome::Tested(report)) => {
                model.set_status(report.summary().into());
                model.set_identity(report.certificate_identity.into());
            }
            Err(error) => model.set_status(format!("Connection unavailable: {error}").into()),
        }
    });
}
pub fn interrupt() {
    crate::remote_jobs::interrupt();
    let window = RUNTIME.with(|value| {
        if let Some(runtime) = value.borrow_mut().as_mut() {
            if runtime.installing {
                return None;
            }
            if let Some((_, cancel)) = runtime.active.take() {
                cancel.store(true, Ordering::Release);
                return Some(runtime.window.clone());
            }
        }
        None
    });
    if let Some(window) = window.and_then(|weak| weak.upgrade()) {
        let model = window.global::<RemoteModel>();
        model.set_busy(false);
        model.set_token("".into());
        model.set_status("Connection interrupted. Retry when ready.".into());
    }
}
pub fn shutdown() {
    interrupt();
    crate::remote_jobs::shutdown();
    RUNTIME.with(|value| drop(value.borrow_mut().take()));
}
