//! The high-resolution export flow: resolves the export directory (prompting once if
//! unset), validates the request, captures a `SceneSnapshot` independent of the live
//! viewport's `RenderContext.width`/`height` and accumulation buffer, fans it out into
//! one render per selected export-usable preset plus the base current-view render, and
//! runs them one at a time via `export_thread::spawn_export`, each resolved to its own
//! filename through the configurable template.
//!
//! Split into [`queue`] (the `ExportQueue`/`ExportJob` engine and its pure
//! scene-transform helpers) and [`wiring`] (the Slint callback registrations).
//!
//! A fan-out export runs its jobs sequentially, not concurrently: `GpuBackend` is a
//! single shared adapter behind a `Mutex`, so two exports racing for it would just
//! serialize on that lock anyway, with none of the benefit and all of the complexity
//! of real parallelism. Running one job at a time also keeps this module's state
//! trivial: exactly one `ExportHandle` is ever "the current one" cancel needs to
//! reach, and exactly one set of `RenderContext.export_active`/preview properties is
//! ever live. The cost is wall-clock time, unavoidable regardless of scheduling -- see
//! [`queue::ExportQueue`]'s doc comment for how progress across the whole queue is
//! reported so that cost doesn't look like a stall.
//!
//! `ExportQueue` lives behind `Arc<Mutex<_>>`, not `Rc<RefCell<_>>`: `spawn_export`'s
//! `on_progress`/`on_done` closures must be `Send` (captured by the export worker's
//! `thread::spawn`, even though actually invoked back on the UI thread via
//! `upgrade_in_event_loop`), so anything they capture must be `Send` too. Every field
//! `ExportQueue` holds (paths, strings, `SceneSnapshot`s, `ExportHandle`) already is.

mod queue;
mod wiring;

pub(in crate::gui) use wiring::setup_render_export_callbacks;
