//! The Light tab's HDR Map section: import a Radiance panorama, clear it, and choose it as
//! the lighting.
//!
//! Decoding, downscaling and writing the stored copy take seconds, so they run on a worker
//! thread; only installing the result touches the app state, back on the UI thread.

use std::sync::Arc;

use indicatrix_mobile_core::{
    hdr::{self, PreparedHdr},
    platform::{AppDirs, OpenKind, OpenRequest, PickedFile},
};
use slint::ComponentHandle;

use super::{HANDLE, bind0, bind1, invalidate};
use crate::{
    AppWindow, PropsModel,
    design::{Shared, refresh},
    push::toast,
};

/// Connects the HDR Map section's callbacks and restores the map from the app's files.
pub(super) fn wire(window: &AppWindow, state: &Shared) {
    let props = window.global::<PropsModel>();
    props.on_hdr_import(bind0(window, state, import));
    props.on_hdr_clear(bind0(window, state, clear));
    props.on_hdr_use(bind1(window, state, use_map));
    hdr::restore(&mut state.borrow_mut());
    load_in_background(window, state);
}

/// Loads the stored map on a worker when it is the chosen lighting, so the first render
/// does not wait for it.
fn load_in_background(window: &AppWindow, state: &Shared) {
    let loader = {
        let app = state.borrow();
        if app.hdr.is_selected() {
            app.hdr.loader()
        } else {
            None
        }
    };
    let Some(loader) = loader else {
        return;
    };
    let weak = window.as_weak();
    let spawned = std::thread::Builder::new()
        .name("indicatrix-hdr-load".to_owned())
        .spawn(move || {
            loader.run();
            let queued = weak.upgrade_in_event_loop(|window| {
                if let Some(state) = HANDLE.with(|handle| handle.borrow().clone()) {
                    refresh(&window, &state);
                }
            });
            if queued.is_err() {
                tracing::debug!("the window closed before the HDR map finished loading");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "the HDR map will load when it is first needed");
    }
}

/// "Import": asks the platform for one `.hdr` file, then prepares it on a worker.
fn import(window: &AppWindow, state: &Shared) {
    let platform = Arc::clone(state.borrow().platform());
    let dirs = platform.app_dirs().clone();
    let weak = window.as_weak();
    platform.open_documents(
        OpenRequest {
            kind: OpenKind::HdrMap,
            allow_multiple: false,
        },
        Box::new(move |result| match result {
            Err(error) => say(&weak, error.message),
            Ok(files) => {
                // An empty list means the user backed out.
                if let Some(file) = files.into_iter().next() {
                    prepare_in_background(&weak, dirs, file);
                }
            }
        }),
    );
}

/// Shows `text` as a toast from any thread.
fn say(weak: &slint::Weak<AppWindow>, text: String) {
    let queued = weak.upgrade_in_event_loop(move |window| toast::show(&window, &text));
    if queued.is_err() {
        tracing::debug!("the window closed before a message could be shown");
    }
}

/// Runs [`hdr::prepare`] on a worker and hands the result to [`finish_import`].
fn prepare_in_background(weak: &slint::Weak<AppWindow>, dirs: AppDirs, file: PickedFile) {
    let busy = weak.upgrade_in_event_loop(|window| set_busy(&window, true));
    if busy.is_err() {
        return;
    }
    let weak = weak.clone();
    let spawned = std::thread::Builder::new()
        .name("indicatrix-hdr-import".to_owned())
        .spawn({
            let weak = weak.clone();
            move || {
                let prepared = hdr::prepare(&dirs, &file);
                drop(file);
                let queued =
                    weak.upgrade_in_event_loop(move |window| finish_import(&window, prepared));
                if queued.is_err() {
                    tracing::debug!("the window closed before the HDR map was installed");
                }
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "the HDR import thread could not start");
        let message = "The HDR map could not be read: the app could not start a thread.";
        let _ = weak.upgrade_in_event_loop(move |window| {
            set_busy(&window, false);
            toast::show(&window, message);
        });
    }
}

/// The worker's answer, back on the UI thread.
fn finish_import(window: &AppWindow, prepared: Result<PreparedHdr, String>) {
    set_busy(window, false);
    let Some(state) = HANDLE.with(|handle| handle.borrow().clone()) else {
        return;
    };
    match prepared {
        Ok(prepared) => {
            let description = hdr::install(&mut state.borrow_mut(), prepared);
            toast::show(window, &format!("{description} imported and in use."));
        }
        Err(message) => toast::show(window, &message),
    }
    invalidate();
    refresh(window, &state);
}

/// Shows or hides the Import button's busy state.
fn set_busy(window: &AppWindow, busy: bool) {
    window.global::<PropsModel>().set_hdr_busy(busy);
}

/// "Clear": forgets the map.
fn clear(window: &AppWindow, state: &Shared) {
    hdr::clear(&mut state.borrow_mut());
    invalidate();
    refresh(window, state);
}

/// The "HDR Map" row was tapped (`true`), or a preset replaced it (`false`, done in the
/// core by choosing the preset).
fn use_map(window: &AppWindow, state: &Shared, on: bool) {
    let result = hdr::set_selected(&mut state.borrow_mut(), on);
    if let Err(message) = result {
        toast::show(window, &message);
    } else if on {
        // The map may still be on disk only: load it off the UI thread.
        load_in_background(window, state);
    }
    invalidate();
    refresh(window, state);
}
