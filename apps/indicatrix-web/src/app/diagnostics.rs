//! Console diagnostics: the panic hook, a minimal `tracing` subscriber that writes
//! to the browser console, and [`console_note`] for the shell's own lines.
//!
//! The subscriber exists so the shared crates' `tracing` events (the editor's
//! cutting-sheet warning, the solver's diagnostics) are visible in devtools rather
//! than dropped. It records events only (no span tracking), `INFO` and above.

use std::fmt::Write as _;
use tracing::{
    Event, Level, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

/// Installs the panic hook and the console subscriber. Call once, first thing.
pub fn install() {
    // A Rust panic otherwise surfaces as an opaque "unreachable executed" trap.
    console_error_panic_hook::set_once();
    // Fails only if a subscriber is already set (a second call); harmless.
    let _ = tracing::subscriber::set_global_default(ConsoleSubscriber { max: Level::INFO });
}

/// Replaces the (blank) page with `message`, for a failure before the app's own window
/// exists (no WebGL, a canvas that is missing) where nothing else could tell the reader
/// why the page is empty. The message is plain text.
pub fn show_fatal(message: &str) {
    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    let (Some(body), Ok(note)) = (document.body(), document.create_element("div")) else {
        return;
    };
    note.set_text_content(Some(message));
    let _ = note.set_attribute(
        "style",
        "position:fixed;inset:0;display:flex;align-items:center;justify-content:center;\
         padding:24px;box-sizing:border-box;text-align:center;font:16px sans-serif;\
         color:#f1f5f9;background:#1b1b1f;",
    );
    let _ = body.append_child(&note);
}

/// One `indicatrix-web: <message>` line on `console.info`.
pub fn console_note(message: &str) {
    web_sys::console::info_1(&format!("indicatrix-web: {message}").into());
}

/// One `indicatrix-web: <message>` line on `console.warn`.
pub fn console_warn(message: &str) {
    web_sys::console::warn_1(&format!("indicatrix-web: {message}").into());
}

/// Writes every enabled event to the console method matching its level.
struct ConsoleSubscriber {
    /// The most verbose level written.
    max: Level,
}

/// Collects an event's `message` and its other fields into one line.
struct LineVisitor(String);

impl Visit for LineVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        } else {
            let _ = write!(self.0, " {}={value}", field.name());
        }
    }
}

impl Subscriber for ConsoleSubscriber {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        *metadata.level() <= self.max
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        // Spans are not tracked; every span shares one id.
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let metadata = event.metadata();
        let mut line = LineVisitor(format!("[{}] {}: ", metadata.level(), metadata.target()));
        event.record(&mut line);
        let text = wasm_bindgen::JsValue::from_str(&line.0);
        match *metadata.level() {
            Level::ERROR => web_sys::console::error_1(&text),
            Level::WARN => web_sys::console::warn_1(&text),
            Level::INFO => web_sys::console::info_1(&text),
            _ => web_sys::console::debug_1(&text),
        }
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}
