//! One Worker: spawn from [`crate::WORKER_LOADER_URL`], post postcard bytes in a
//! transferred `ArrayBuffer`, decode replies.

use std::{cell::RefCell, rc::Rc};

use js_sys::{Array, ArrayBuffer, Uint8Array};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use web_sys::{ErrorEvent, MessageEvent, Worker};

use crate::{
    WORKER_LOADER_URL,
    protocol::{FromWorker, ToWorker, decode_from_worker, encode_to_worker},
};

/// A live Worker and the two JS closures bound to it. Dropping the handle drops the
/// closures, so terminate it first ([`Self::terminate`]) and never drop it from inside
/// its own callbacks (the pools park retired handles until a later call).
pub(super) struct WorkerHandle {
    worker: Worker,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_error: Closure<dyn FnMut(ErrorEvent)>,
}

impl WorkerHandle {
    /// Starts a Worker. `on_message` gets every decoded reply; `on_error` gets a
    /// description of an undecodable reply or of the Worker's own `error` event (a
    /// script load failure or an uncaught panic).
    pub(super) fn spawn(
        mut on_message: impl FnMut(FromWorker) + 'static,
        on_error: impl FnMut(String) + 'static,
    ) -> Result<Self, String> {
        let worker = Worker::new(WORKER_LOADER_URL).map_err(|e| {
            format!(
                "could not start a worker from {WORKER_LOADER_URL}: {}",
                js_error_text(&e)
            )
        })?;
        let on_error = Rc::new(RefCell::new(on_error));
        let decode_error = Rc::clone(&on_error);
        let on_message_closure = Closure::<dyn FnMut(MessageEvent)>::new(
            move |event: MessageEvent| match decode_event(&event) {
                Ok(message) => on_message(message),
                Err(error) => (decode_error.borrow_mut())(error),
            },
        );
        worker.set_onmessage(Some(on_message_closure.as_ref().unchecked_ref()));
        let on_error_closure = Closure::<dyn FnMut(ErrorEvent)>::new(move |event: ErrorEvent| {
            event.prevent_default();
            (on_error.borrow_mut())(format!(
                "worker failed: {} ({}:{})",
                event.message(),
                event.filename(),
                event.lineno()
            ));
        });
        worker.set_onerror(Some(on_error_closure.as_ref().unchecked_ref()));
        Ok(Self {
            worker,
            _on_message: on_message_closure,
            _on_error: on_error_closure,
        })
    }

    /// Posts `message`, transferring its buffer.
    pub(super) fn post(&self, message: &ToWorker) -> Result<(), String> {
        let bytes = encode_to_worker(message)?;
        let array = Uint8Array::new_with_length(bytes.len() as u32);
        array.copy_from(&bytes);
        let buffer = array.buffer();
        self.worker
            .post_message_with_transfer(&buffer, &Array::of1(&buffer))
            .map_err(|e| format!("posting to a worker: {}", js_error_text(&e)))
    }

    /// Stops the Worker at once and unhooks its callbacks.
    pub(super) fn terminate(&self) {
        self.worker.set_onmessage(None);
        self.worker.set_onerror(None);
        self.worker.terminate();
    }
}

fn decode_event(event: &MessageEvent) -> Result<FromWorker, String> {
    let buffer: ArrayBuffer = event
        .data()
        .dyn_into()
        .map_err(|_| "a worker reply was not an ArrayBuffer".to_string())?;
    decode_from_worker(&Uint8Array::new(&buffer).to_vec())
}

/// A readable text for a thrown JS value.
pub(super) fn js_error_text(value: &JsValue) -> String {
    value.as_string().unwrap_or_else(|| {
        value
            .dyn_ref::<js_sys::Error>()
            .map_or_else(|| format!("{value:?}"), |e| String::from(e.message()))
    })
}
