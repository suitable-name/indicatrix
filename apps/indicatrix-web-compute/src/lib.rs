//! The Web Worker entry point of the indicatrix browser app.
//!
//! Trunk builds this crate as a worker (`data-type="worker"` in
//! `apps/indicatrix-web/index.html`): wasm-bindgen's `no-modules` output plus a loader
//! shim, `indicatrix-web-compute_loader.js`, which the page starts with
//! `new Worker(...)` (see `indicatrix_web_core::WORKER_LOADER_URL`).
//!
//! On start the Worker installs its `onmessage` handler, then posts `Loaded`. Each
//! message is a postcard-encoded `ToWorker` in a transferred `ArrayBuffer`; it is
//! handed to `indicatrix_web_core::worker::WorkerHandler`, which owns every decision
//! (and the cached scene, plane arena and HDR map, kept across chunks), and the reply
//! goes back the same way. A long solve job's progress reports are posted as they are
//! emitted, before the reply. Time is `performance.now()` from the Worker's own global.
//!
//! Like `apps/indicatrix-web`, everything is `#[cfg(target_arch = "wasm32")]`, so a
//! native build of the workspace compiles this crate to nothing.

#[cfg(target_arch = "wasm32")]
mod entry {
    use std::cell::{Cell, RefCell};

    use indicatrix_web_core::{
        protocol::{FromWorker, decode_to_worker, encode_from_worker},
        worker::{CancelProbe, WorkerHandler},
    };
    use js_sys::{Array, ArrayBuffer, Uint8Array};
    use wasm_bindgen::{JsCast, closure::Closure, prelude::wasm_bindgen};
    use web_sys::{DedicatedWorkerGlobalScope, MessageEvent};

    thread_local! {
        static HANDLER: RefCell<WorkerHandler> = RefCell::new(WorkerHandler::new());
        /// Whether this Worker can make a synchronous request to a `blob:` URL it made
        /// itself; found out the first time a request to the page's cancel URL fails.
        static OWN_BLOB_REQUEST_WORKS: Cell<Option<bool>> = const { Cell::new(None) };
    }

    fn scope() -> DedicatedWorkerGlobalScope {
        js_sys::global().unchecked_into()
    }

    fn now_ms() -> f64 {
        scope().performance().map_or(0.0, |p| p.now())
    }

    fn post(message: &FromWorker) {
        let bytes = match encode_from_worker(message) {
            Ok(bytes) => bytes,
            Err(error) => {
                web_sys::console::error_1(&error.into());
                return;
            }
        };
        let array = Uint8Array::new_with_length(bytes.len() as u32);
        array.copy_from(&bytes);
        let buffer = array.buffer();
        if let Err(error) = scope().post_message_with_transfer(&buffer, &Array::of1(&buffer)) {
            web_sys::console::error_2(&"indicatrix worker: postMessage failed".into(), &error);
        }
    }

    /// How one synchronous `GET` of a `blob:` URL ended.
    enum Attempt {
        /// The URL was served.
        Served,
        /// The request was made and failed (a revoked URL fails like this).
        Failed,
        /// No request could be made, or it reported no status at all.
        CouldNotRun,
    }

    fn attempt(url: &str) -> Attempt {
        let Ok(request) = web_sys::XmlHttpRequest::new() else {
            return Attempt::CouldNotRun;
        };
        if request.open_with_async("GET", url, false).is_err() {
            return Attempt::CouldNotRun;
        }
        match request.send() {
            Ok(()) => match request.status() {
                Ok(0) | Err(_) => Attempt::CouldNotRun,
                Ok(_) => Attempt::Served,
            },
            Err(_) => Attempt::Failed,
        }
    }

    /// Whether this Worker can request a `blob:` URL at all: makes one of its own,
    /// requests it and revokes it. Asked once and remembered.
    fn own_blob_request_works() -> bool {
        if let Some(known) = OWN_BLOB_REQUEST_WORKS.with(Cell::get) {
            return known;
        }
        let works = web_sys::Blob::new()
            .ok()
            .and_then(|blob| web_sys::Url::create_object_url_with_blob(&blob).ok())
            .is_some_and(|own| {
                let served = matches!(attempt(&own), Attempt::Served);
                let _ = web_sys::Url::revoke_object_url(&own);
                served
            });
        OWN_BLOB_REQUEST_WORKS.with(|cell| cell.set(Some(works)));
        works
    }

    /// What the page's cancel URL says about the running job: `url` is a `blob:` URL the
    /// page made for it and revokes to cancel it (`ToWorker::WatchCancel`). A Worker
    /// cannot read a message while it computes, but it can make a SYNCHRONOUS request,
    /// and a request for a revoked `blob:` URL fails.
    ///
    /// A strict content-security policy (`connect-src` without `blob:`) makes the same
    /// request fail, so a failure only counts as a revocation when a URL this Worker made
    /// itself can be requested. Anything else that stops the probe running (no request
    /// object, `open` refused, no status in the answer) is `Unavailable`: the job then
    /// carries on instead of being stopped by a probe that could not run.
    fn cancel_url_probe(url: &str) -> CancelProbe {
        match attempt(url) {
            Attempt::Served => CancelProbe::Live,
            Attempt::CouldNotRun => CancelProbe::Unavailable,
            Attempt::Failed => {
                if own_blob_request_works() {
                    CancelProbe::Revoked
                } else {
                    CancelProbe::Unavailable
                }
            }
        }
    }

    fn handle_bytes(bytes: &[u8]) -> Option<FromWorker> {
        match decode_to_worker(bytes) {
            // A long job (an Optimize or Retarget search, a render chunk) may post its
            // progress while it computes: the Worker cannot read messages then, but it
            // can post them, and it can look at its cancel URL between tier decisions
            // (between row groups).
            Ok(message) => HANDLER.with(|handler| {
                handler.borrow_mut().handle_probed(
                    message,
                    &now_ms,
                    &|streamed| post(&streamed),
                    &cancel_url_probe,
                )
            }),
            Err(message) => Some(FromWorker::Error { message }),
        }
    }

    fn on_message(event: &MessageEvent) {
        let reply = event.data().dyn_into::<ArrayBuffer>().map_or_else(
            |_| {
                Some(FromWorker::Error {
                    message: "a worker message was not an ArrayBuffer".to_string(),
                })
            },
            |buffer| handle_bytes(&Uint8Array::new(&buffer).to_vec()),
        );
        if let Some(reply) = reply {
            post(&reply);
        }
    }

    /// Runs when the module is instantiated: hook up `onmessage`, then say so.
    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
        let closure = Closure::<dyn FnMut(MessageEvent)>::new(|event: MessageEvent| {
            on_message(&event);
        });
        scope().set_onmessage(Some(closure.as_ref().unchecked_ref()));
        // The handler lives as long as the Worker.
        closure.forget();
        post(&WorkerHandler::loaded_message());
    }
}
