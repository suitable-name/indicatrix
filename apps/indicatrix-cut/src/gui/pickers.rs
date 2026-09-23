//! Off-UI-thread native file pickers -- the ONE place `rfd::FileDialog` is ever
//! constructed anywhere in this app, so no picker dialog ever blocks the UI
//! thread.
//!
//! Generalizes `gui::editor::native_io`'s own `pick_file`/`PickKind`
//! (that module moved its own dialogs off the UI thread, but left every other
//! module's `rfd` call untouched -- see this module's own handoff note) into
//! one shared worker every picker in the app goes through:
//! [`pick`] shows [`PickerRequest`]'s dialog on a background thread (never the
//! UI thread) and delivers the result to a continuation on the UI thread via
//! `Weak::upgrade_in_event_loop`, plus a `#[cfg(test)]` hook
//! ([`set_test_pick_answer`]) so a test can inject a chosen path (or a
//! cancellation) with no real OS dialog and no spawned thread.
//!
//! # Why a request enum/struct, not a caller-supplied `rfd::FileDialog` builder
//!
//! A closure like `impl FnOnce() -> rfd::FileDialog` would still need `rfd::`
//! written out at every call site to build one, defeating the point of having
//! a single construction site to grep for. Every
//! picker's own shape (open/save/folder, filters, starting directory, default
//! file name) is instead plain data ([`PickerRequest`]), and only [`pick`]
//! itself (via the private [`build_dialog`]) ever turns that into a real
//! `rfd::FileDialog`.
//!
//! # `gui::editor::native_io`
//!
//! That module's own `pick_file`/`PickKind` are kept (not removed) as a thin
//! wrapper over [`pick`]/[`PickerRequest`] -- see that module's own doc
//! comment on `pick_file` for why: five call sites there already build a
//! `PickKind` value and thread a continuation through it, and none of that
//! shape needs to change now that the picker itself lives here instead.
//!
//! # Every `rfd`-using dialog in this app goes through here
//!
//! `gui::render::camera_lighting::setup_environment_map_callbacks`'s
//! `on_pick_hdr_file` and `gui::remote::worker_callbacks::setup_cert_dir_picker_callback`'s
//! `on_pick_cert_dir` are the two dialogs whose own Slint callback signature had
//! to change to reach this module: both were Slint callbacks that
//! RETURNED their result directly into a property assignment
//! (`root.env_map_path = root.pick_hdr_file(root.env_map_path)`,
//! `root.form_cert_dir = RemoteWorkerModel.pick_cert_dir(root.form_cert_dir)`)
//! -- Slint has no way to await a value-returning callback, so neither could
//! move here while their own signatures stayed value-returning. Both callbacks
//! are void now (`ui/models/settings.slint`/`ui/models/remote_worker.slint`), and each
//! dialog (`settings_dialog.slint`'s `env_map_path`,
//! `remote_worker_dialog.slint`'s `form_cert_dir` via a new
//! `RemoteWorkerModel.picked_cert_dir` property and `changed` handler) is
//! filled from the completion continuation this module hands back on the UI
//! thread instead -- so both now build their `PickerRequest` and call [`pick`]
//! exactly like every other caller.

use crate::MainWindow;
use slint::{ComponentHandle, Weak};
use std::{cell::RefCell, collections::HashMap, path::PathBuf};

/// One filter row a picker offers -- a label plus the extensions (no leading
/// dot) it accepts. Mirrors `rfd::FileDialog::add_filter`'s own two
/// parameters exactly; [`build_and_show`] is the only place this is ever
/// turned into a real one.
///
/// Owned (`String`/`Vec<String>`), not `&'static str`/`&'static [&'static str]`:
/// most of this app's own filters ARE `'static` literals (a fixed extension
/// like `"asc"`), but at least one real caller
/// (`gui::library::detail::export_diagram_file_via_source`) needs a filter
/// built from a RUNTIME string (an attachment's own file extension, not known
/// until the attachment is picked) -- a `'static`-only field would force that
/// caller to leak memory (`Box::leak`) just to satisfy the type, once per
/// click, for no real benefit over an owned `String` a `PickerRequest` already
/// gets dropped after one use anyway.
pub(super) struct PickerFilter {
    pub label: String,
    pub extensions: Vec<String>,
}

impl PickerFilter {
    /// The common case: a single extension whose own name (with no leading
    /// dot) doubles as the filter's label -- e.g. `PickerFilter::single("asc")`
    /// reads as `label: "asc", extensions: ["asc"]`. Most of this app's own
    /// filters are exactly this shape; a caller that wants a different label
    /// (e.g. `".asc design"` for `"asc"`) builds the struct literal directly.
    #[must_use]
    pub fn single(extension: impl Into<String>) -> Self {
        let extension = extension.into();
        Self {
            extensions: vec![extension.clone()],
            label: extension,
        }
    }
}

/// Which native dialog [`pick`] should show.
pub(super) enum PickerKind {
    /// A single-file "Open" picker.
    OpenFile,
    /// A single-file "Save As" picker.
    SaveFile,
    /// A folder picker.
    PickFolder,
}

/// Everything one native dialog needs, as plain data -- every
/// `rfd::FileDialog` builder method this app's own dialogs use, named as a
/// field instead of a method call. See the module
/// doc comment for why this exists at all (not a caller-supplied
/// `rfd::FileDialog` builder closure).
pub(super) struct PickerRequest {
    pub kind: PickerKind,
    /// `rfd::FileDialog::set_title`. `None` leaves the OS's own default title.
    pub title: Option<String>,
    /// `rfd::FileDialog::add_filter`, applied in order -- the FIRST filter is
    /// the dialog's own default/leading one, matching `rfd`'s own convention.
    pub filters: Vec<PickerFilter>,
    /// `rfd::FileDialog::set_file_name` -- a `SaveFile` picker's suggested
    /// name. Ignored for `OpenFile`/`PickFolder`.
    pub default_file_name: Option<String>,
    /// `rfd::FileDialog::set_directory` -- the starting directory. `None`
    /// leaves the OS's own last-used-directory memory in place.
    pub starting_dir: Option<PathBuf>,
}

/// [`set_test_pick_answer`]'s own stashed answer -- a one-field wrapper around
/// `Option<PathBuf>` (the picker's own "cancelled or picked" result) purely so
/// the OUTER "is a test answer stashed at all" state can be a single-level
/// `Option`, not `Option<Option<PathBuf>>` (`clippy::option_option`) -- the
/// same shape `gui::editor::native_io::TestPickAnswer` uses.
struct TestPickAnswer(Option<PathBuf>);

/// A stashed [`pick`] continuation -- named purely to keep [`PENDING_PICKS`]'s
/// own type under clippy's `type_complexity` lint.
type PickContinuation = Box<dyn FnOnce(&MainWindow, Option<PathBuf>)>;

thread_local! {
    /// Test hook: when `Some`, the NEXT [`pick`] call resolves to this answer
    /// with no OS dialog and no spawned thread, consumed (taken) exactly once.
    /// `TestPickAnswer(None)` stubs a dismissed picker.
    static TEST_PICK_ANSWER: RefCell<Option<TestPickAnswer>> = const { RefCell::new(None) };
    /// [`pick`]'s own UI-thread-only rendezvous for its (typically non-`Send`)
    /// `on_done` continuation: a picker's own caller routinely closes over an
    /// `Rc<RefCell<EditorState>>`-shaped handle, which must never itself cross
    /// into the spawned thread below or into the `upgrade_in_event_loop`
    /// closure that hops back (both require `Send`). Only a plain `u64` key
    /// and the `Send`-safe `Option<PathBuf>` answer ever actually cross the
    /// thread boundary; `on_done` itself is inserted and removed on the UI
    /// thread only.
    static PENDING_PICKS: RefCell<HashMap<u64, PickContinuation>> = RefCell::new(HashMap::new());
    static NEXT_PICK_KEY: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Test-only: see [`TEST_PICK_ANSWER`]'s own doc comment.
#[cfg(test)]
pub(super) fn set_test_pick_answer(answer: Option<PathBuf>) {
    TEST_PICK_ANSWER.with(|cell| *cell.borrow_mut() = Some(TestPickAnswer(answer)));
}

/// Pure precedence check, split out of [`pick`] purely so a test can exercise
/// the hook without a live `MainWindow`: `Some` (consuming the stashed answer)
/// when a test has stubbed one, `None` otherwise (the ordinary "show a real
/// picker" path).
fn take_test_pick_answer() -> Option<TestPickAnswer> {
    TEST_PICK_ANSWER.with(|cell| cell.borrow_mut().take())
}

/// Turns a [`PickerRequest`] into a real `rfd::FileDialog` and shows it,
/// returning the chosen path (`None` on cancel/dismiss) -- the ONLY function
/// in this app that constructs one. Always called from a background thread
/// (see [`pick`]), never the UI thread.
fn build_and_show(request: &PickerRequest) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new();
    if let Some(title) = &request.title {
        dialog = dialog.set_title(title);
    }
    for filter in &request.filters {
        let extensions: Vec<&str> = filter.extensions.iter().map(String::as_str).collect();
        dialog = dialog.add_filter(&filter.label, &extensions);
    }
    if let Some(name) = &request.default_file_name {
        dialog = dialog.set_file_name(name);
    }
    if let Some(dir) = &request.starting_dir {
        dialog = dialog.set_directory(dir);
    }
    match request.kind {
        PickerKind::OpenFile => dialog.pick_file(),
        PickerKind::SaveFile => dialog.save_file(),
        PickerKind::PickFolder => dialog.pick_folder(),
    }
}

/// The one entry point every native file/folder picker in this app uses: shows
/// `request`'s dialog on a background thread (never the UI thread) and
/// delivers the result (`None` on cancel/dismiss) to `on_done` on the UI
/// thread via `Weak::upgrade_in_event_loop`. The caller's own state must never
/// be borrowed across a call to this function -- pick first, then borrow,
/// never the other way around (the same discipline `gui::editor::native_io`'s
/// own former `pick_file` doc comment already established). `on_done` silently
/// never runs if the window closed while the picker was open, the same
/// convention every worker in this crate follows.
pub(super) fn pick(
    ui: &MainWindow,
    request: PickerRequest,
    on_done: impl FnOnce(&MainWindow, Option<PathBuf>) + 'static,
) {
    if let Some(TestPickAnswer(answer)) = take_test_pick_answer() {
        on_done(ui, answer);
        return;
    }
    let key = NEXT_PICK_KEY.with(|c| {
        let key = c.get();
        c.set(key + 1);
        key
    });
    PENDING_PICKS.with(|cell| {
        cell.borrow_mut().insert(key, Box::new(on_done));
    });
    let ui_weak: Weak<MainWindow> = ui.as_weak();
    std::thread::spawn(move || {
        let picked = build_and_show(&request);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            if let Some(on_done) = PENDING_PICKS.with(|cell| cell.borrow_mut().remove(&key)) {
                on_done(&ui, picked);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::{TEST_PICK_ANSWER, TestPickAnswer, set_test_pick_answer, take_test_pick_answer};
    use std::path::PathBuf;

    /// The picker test hook's own take/set mechanics, moved here from
    /// `gui::editor::native_io`'s `pick_file_test_hook_is_consumed_exactly_once`
    /// test when the picker itself generalized to this module -- same coverage,
    /// same assertions.
    #[test]
    fn pick_test_hook_is_consumed_exactly_once() {
        // No hook stashed yet -- `pick` must fall through to a real picker
        // (not exercised here, since this suite has no display -- see
        // `solve_service.rs`'s own tests for the same constraint). Cleared
        // first so this test is independent of whatever another test in this
        // same process left behind in the shared `thread_local!`.
        TEST_PICK_ANSWER.with(|cell| *cell.borrow_mut() = None);
        assert!(take_test_pick_answer().is_none());

        set_test_pick_answer(Some(PathBuf::from("chosen.asc")));
        let answer = take_test_pick_answer().expect("a stashed answer must be returned");
        assert_eq!(answer.0, Some(PathBuf::from("chosen.asc")));
        // Consumed: a second take before another `set` sees nothing stashed.
        assert!(take_test_pick_answer().is_none());

        // `Some(None)` stubs a dismissed picker -- distinct from "no hook at all".
        set_test_pick_answer(None);
        assert_eq!(take_test_pick_answer().expect("stashed").0, None);
    }

    /// [`TestPickAnswer`] is only ever constructed via [`take_test_pick_answer`]'s
    /// own pattern match above -- referenced here purely so a future refactor
    /// that renames its single field doesn't silently go unnoticed by every
    /// test in this module at once.
    #[test]
    fn test_pick_answer_wraps_the_answer_directly() {
        assert_eq!(TestPickAnswer(None).0, None);
    }
}
