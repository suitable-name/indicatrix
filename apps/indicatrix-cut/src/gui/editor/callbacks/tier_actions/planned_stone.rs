//! A stone the Rough Planner hands to the editor: the design is loaded through the normal
//! Load Selected path (unsaved-changes guard included) and, once it is on the bench, the planned
//! piece, the real width and the adopted colour are applied as ONE undoable edit.
//!
//! # The flow
//!
//! 1. [`open`] remembers the request, selects the design in the library and invokes
//!    `EditorModel.load_selected`, exactly what the Load Selected button does. The editor's
//!    unsaved-changes guard may ask first; its Cancel drops the request ([`clear`]).
//! 2. When the design lands on the bench, `lifecycle::apply_loaded_design` asks
//!    [`take_for`] for the request of that very design (any other load drops it) and calls
//!    [`apply_pending`].
//! 3. [`apply_pending`] points the render at the adopted colour material and
//!    applies the material, the stone's real width (the girdle diameter) and the preform with
//!    [`apply_explicit_edits`]: one history entry, so one Undo takes the whole stone back.
//!
//! The planner's two actions share this path, so a stone's colour lands on the design it was
//! computed for and not on whatever design happened to be open.

use super::materials_symmetry::{ExplicitChoice, apply_explicit_edits};
use crate::{
    EditorModel, LibraryModel, MainWindow, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{state::EditorState, view::SolidLastSolved},
        render::material::ExplicitApply,
        rough_colour::{adopt_link, store::AdoptOutcome},
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
        startup_settings::find_option_index,
    },
};
use indicatrix_cut_core::PreformSpec;
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{Arc, Mutex},
};

/// A planned stone to open in the editor.
pub(in crate::gui) struct PlannedStoneRequest {
    /// The library entry of the stone's design.
    pub entry_id: i64,
    /// The design's title, for the toast.
    pub title: String,
    /// The piece of rough as the design's preform (`None`: the width and colour only).
    pub preform: Option<PreformSpec>,
    /// The stone's real width, the girdle diameter, in mm.
    pub girdle_mm: f64,
    /// The adopted colour material (plans with a stored colour).
    pub material: Option<AdoptOutcome>,
    /// The catalogue database, to rebuild the custom-material list that holds the adopted
    /// material.
    pub db: Arc<Mutex<Database>>,
    /// The result the stone is in, from 1 (for the toast).
    pub result_number: usize,
    /// The stone's place in the result, from 1 (for the toast).
    pub stone_number: usize,
}

thread_local! {
    /// The request waiting for its design to land on the bench: set by [`open`], taken by
    /// [`take_for`] and dropped by [`clear`] or by any other load.
    static PENDING: RefCell<Option<PlannedStoneRequest>> = const { RefCell::new(None) };
    /// Set while [`open`] itself invokes Load Selected, so the handler can tell its own load from
    /// one the person started.
    static OPENING: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` with [`OPENING`] set (see [`load_selected_invoked`]).
fn while_opening<R>(f: impl FnOnce() -> R) -> R {
    OPENING.with(|flag| flag.set(true));
    let result = f();
    OPENING.with(|flag| flag.set(false));
    result
}

/// Starts the load: shows the Edit view (the unsaved-changes guard is asked there), remembers
/// the request, selects the design in the library and invokes Load Selected. The guard may ask
/// first; a Cancel drops the request ([`clear`]).
///
/// Refused with a toast, and nothing remembered, when the entry is beyond the library's `int`
/// range or the library does not select it (the design is gone or unreadable): Load Selected
/// would otherwise load whatever design was selected before.
pub(in crate::gui) fn open(ui: &MainWindow, request: PlannedStoneRequest) {
    let Ok(id) = i32::try_from(request.entry_id) else {
        show_toast(
            ui,
            "This design is beyond the library's id range, so it cannot be opened.",
            "error",
        );
        return;
    };
    crate::gui::commands::show_edit_view(ui);
    let library = ui.global::<LibraryModel>();
    library.invoke_select_diagram(id);
    if library.get_selected_entry_id() != id {
        show_toast(
            ui,
            "The library could not select this design, so it was not opened.",
            "error",
        );
        return;
    }
    PENDING.with(|cell| *cell.borrow_mut() = Some(request));
    while_opening(|| ui.global::<EditorModel>().invoke_load_selected());
}

/// The pending request for the design that just landed on the bench, if it is the one asked
/// for. `source_entry_id` is the catalogue row the design was loaded from (`Some` only for a
/// local load); any other load drops the request, so it never waits for a later one.
pub(super) fn take_for(source_entry_id: Option<i64>) -> Option<PlannedStoneRequest> {
    let request = PENDING.with(|cell| cell.borrow_mut().take())?;
    (source_entry_id == Some(request.entry_id)).then_some(request)
}

/// Drops a pending request (the unsaved-changes guard was cancelled, or the load failed).
pub(super) fn clear() {
    PENDING.with(|cell| *cell.borrow_mut() = None);
}

/// Forgets a request a previous main window left waiting (it carries the catalogue database).
/// See `gui::window_release`.
pub(in crate::gui) fn release_window_state() {
    let old = PENDING.with(|cell| cell.borrow_mut().take());
    drop(old);
    OPENING.with(|flag| flag.set(false));
}

/// `EditorModel.load_selected` was invoked: unless it is [`open`]'s own call, the person started
/// a load, and a request left behind (a Save that never landed, say) must not wait for it.
pub(super) fn load_selected_invoked() {
    if !OPENING.with(Cell::get) {
        clear();
    }
}

/// The editor's handles one apply needs, bundled to keep the argument counts down.
struct Bench<'a> {
    /// The main window.
    ui: &'a MainWindow,
    /// The editor's state (the design on the bench).
    state: &'a Rc<RefCell<EditorState>>,
    /// The render context the preview traces.
    render_ctx: &'a Arc<Mutex<RenderContext>>,
    /// The Solid preview worker's state.
    preview_state: &'a Arc<SolidPreviewState>,
    /// The masts of the last solve.
    solid_last_solved: &'a SolidLastSolved,
}

/// Applies the width and the preform of `request`, and the material `name` when there is one, to
/// the design on the bench as one edit.
fn apply_edits(
    bench: &Bench<'_>,
    request: &PlannedStoneRequest,
    name: Option<&str>,
) -> ExplicitApply {
    apply_explicit_edits(
        bench.ui,
        bench.state,
        bench.render_ctx,
        bench.preview_state,
        bench.solid_last_solved,
        &ExplicitChoice {
            name,
            width_mm: Some(request.girdle_mm),
            preform: request.preform.as_ref(),
        },
    )
}

/// [`apply_edits`] with the adopted colour: the render is pointed at the material first
/// (`adopt_link::point_render_at`) and put back when the design takes nothing. Returns what
/// happened and the name of the material that was applied, if any.
fn apply_with_colour<'a>(
    bench: &Bench<'_>,
    request: &'a PlannedStoneRequest,
) -> (ExplicitApply, Option<&'a str>) {
    let Some(outcome) = request.material.as_ref() else {
        return (apply_edits(bench, request, None), None);
    };
    match adopt_link::point_render_at(bench.ui, &request.db, outcome) {
        Ok(previous) => {
            let applied = apply_edits(bench, request, Some(&outcome.material_name));
            if applied != ExplicitApply::Design {
                // The refusal was toasted; the picture must keep tracing the material the
                // design still has.
                adopt_link::restore(previous);
            }
            (applied, Some(outcome.material_name.as_str()))
        }
        Err(message) => {
            show_toast(bench.ui, &message, "error");
            (apply_edits(bench, request, None), None)
        }
    }
}

/// Selects the material called `name` in the viewport's material list, as the material editor's
/// "Save and apply" does.
fn select_in_viewport(ui: &MainWindow, name: &str) {
    let viewport = ui.global::<ViewportModel>();
    if let Some(index) = find_option_index(&viewport.get_material_options(), name) {
        viewport.set_selected_material_index(index);
    }
}

/// The toast that says what was applied: the preform from the rough (when there is one), the
/// width, and the colour material (when there is one).
fn success_text(request: &PlannedStoneRequest, material: Option<&str>) -> String {
    let PlannedStoneRequest {
        title,
        preform,
        girdle_mm,
        result_number,
        stone_number,
        ..
    } = request;
    let applied = match (preform.is_some(), material) {
        (true, _) => format!("preform from the rough, width {girdle_mm:.2} mm"),
        (false, Some(_)) => format!("width {girdle_mm:.2} mm"),
        (false, None) => format!("width {girdle_mm:.2} mm only"),
    };
    let colour = material.map_or_else(String::new, |name| format!(", color '{name}'"));
    format!(
        "Opened '{title}' as stone {stone_number} of result #{result_number}: {applied}{colour}"
    )
}

/// Applies `request` to the design on the bench: the colour material first (the render is
/// pointed at it, `adopt_link::point_render_at`), then the material, the width and the preform
/// together (`apply_explicit_edits`, one undoable edit). When the design refuses the apply
/// (`Busy`) the render context is put back (`adopt_link::restore`) and the refusal, already
/// toasted, stands; when it takes the stone, a toast says what was applied.
pub(super) fn apply_pending(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    request: &PlannedStoneRequest,
) {
    let bench = Bench {
        ui,
        state,
        render_ctx,
        preview_state,
        solid_last_solved,
    };
    let (applied, material) = apply_with_colour(&bench, request);
    match applied {
        // The refusal was toasted; reporting the stone as opened now would be false.
        ExplicitApply::Busy => {}
        ExplicitApply::Library => show_toast(
            ui,
            "The design is not open in the editor, so the planned stone was not applied.",
            "error",
        ),
        ExplicitApply::Design => {
            if let Some(name) = material {
                select_in_viewport(ui, name);
            }
            show_toast(ui, &success_text(request, material), "success");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(entry_id: i64, preform: Option<PreformSpec>) -> PlannedStoneRequest {
        PlannedStoneRequest {
            entry_id,
            title: "Oval".to_owned(),
            preform,
            girdle_mm: 6.5,
            material: None,
            db: Arc::new(Mutex::new(
                Database::new(Some(":memory:")).expect("an in-memory vault"),
            )),
            result_number: 3,
            stone_number: 1,
        }
    }

    fn pending_entry() -> Option<i64> {
        PENDING.with(|cell| cell.borrow().as_ref().map(|request| request.entry_id))
    }

    #[test]
    fn the_request_is_taken_only_by_the_design_it_names() {
        PENDING.with(|cell| *cell.borrow_mut() = Some(request(41, None)));
        assert!(take_for(Some(41)).is_some());
        assert_eq!(pending_entry(), None, "taking it empties the slot");
        assert!(take_for(Some(41)).is_none(), "it is applied once");
    }

    #[test]
    fn any_other_load_drops_the_request() {
        PENDING.with(|cell| *cell.borrow_mut() = Some(request(41, None)));
        assert!(take_for(Some(42)).is_none());
        assert_eq!(pending_entry(), None);

        // A remote design (no local row) and a file the person opened never match.
        PENDING.with(|cell| *cell.borrow_mut() = Some(request(41, None)));
        assert!(take_for(None).is_none());
        assert_eq!(pending_entry(), None);
    }

    #[test]
    fn cancelling_the_guard_drops_the_request() {
        PENDING.with(|cell| *cell.borrow_mut() = Some(request(41, None)));
        clear();
        assert_eq!(pending_entry(), None);
        assert!(take_for(Some(41)).is_none());
    }

    #[test]
    fn a_load_the_person_starts_drops_a_request_left_behind() {
        PENDING.with(|cell| *cell.borrow_mut() = Some(request(41, None)));
        // The planner's own Load Selected keeps its request.
        while_opening(load_selected_invoked);
        assert_eq!(pending_entry(), Some(41));
        assert!(!OPENING.with(Cell::get), "the flag is down again");
        // The Load Selected button does not.
        load_selected_invoked();
        assert_eq!(pending_entry(), None);
    }

    #[test]
    fn the_toast_says_what_was_applied() {
        let piece = PreformSpec::block(1.0, 1.0, 1.0);
        let with_preform = request(7, Some(piece));
        assert_eq!(
            success_text(&with_preform, None),
            "Opened 'Oval' as stone 1 of result #3: preform from the rough, width 6.50 mm"
        );
        assert_eq!(
            success_text(&with_preform, Some("Melon - Oval color")),
            "Opened 'Oval' as stone 1 of result #3: preform from the rough, width 6.50 mm, \
             color 'Melon - Oval color'"
        );
        let width_only = request(7, None);
        assert_eq!(
            success_text(&width_only, None),
            "Opened 'Oval' as stone 1 of result #3: width 6.50 mm only"
        );
        assert_eq!(
            success_text(&width_only, Some("Melon - Oval color")),
            "Opened 'Oval' as stone 1 of result #3: width 6.50 mm, color 'Melon - Oval color'"
        );
    }
}
