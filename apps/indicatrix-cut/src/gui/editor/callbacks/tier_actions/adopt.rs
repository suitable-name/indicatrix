//! "Adopt" (all/selected) and its inverse "Pin to mast".

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use indicatrix::geometry::meet_solver::{MeetConstraint, SolveStrategy, SolvedTier};
use indicatrix_cut_core::Edit;
use slint::ComponentHandle;

use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            stall_guard::stall_guard,
            state::EditorState,
            view::{
                SolidLastSolved, refresh_all_now, refresh_editor_panel_stale, submit_preview_replan,
            },
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// Bulk-adopts every tier that still has an unadopted
/// `imported_meet`, as one undoable step -- built as a single [`Edit::Batch`] of
/// per-tier [`Edit::SetConstraint`]s (mirroring `solve_actions::
/// setup_adopt_meet_callback`'s own one-tier "Adopt") rather than looping single
/// edits, so Undo reverses the whole thing in one step and only ONE `refresh_all`
/// (one real re-solve) runs afterward instead of one per tier. A silent no-op
/// when nothing has an `imported_meet` left to adopt.
///
/// Calls [`refresh_all`], not `refresh_editor_panel_stale`, for the exact reason
/// `setup_adopt_meet_callback`'s own doc comment gives: every value this adopts
/// is already what the design's last real solve produced, so re-solving here
/// pays the same whole-schedule cost that solve already paid, not a new one.
///
/// Wired to `EditorModel.adopt_all()` (declared in `ui/models/editor.slint`) --
/// `editor_tier_table.slint`'s own "Adopt all" button calls it.
///
/// `pub(super)` since [`super::tier_crud::setup_toggle_detach_callback`] is the
/// one call site that registers it.
pub(super) fn setup_adopt_all_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_adopt_all(move || {
        stall_guard("on_adopt_all", || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // One `Edit::Batch` of `Edit::SetConstraint`s, or nothing when no tier
            // has an imported meet left -- `EditorSession::adopt_all_imported_meets`.
            match st.adopt_all_imported_meets() {
                Ok(0) => {}
                Ok(count) => {
                    drop(st);
                    refresh_all_now(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &state,
                        false,
                    );
                    let plural = if count == 1 { "" } else { "s" };
                    show_toast(
                        &ui,
                        &format!("Adopted {count} imported meet{plural}"),
                        "info",
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
    });
}

/// Like [`setup_adopt_all_callback`] above, but
/// restricted to [`EditorState::multi_selected`] instead of every tier in the
/// design -- freeing one Ctrl-clicked group for Optimize without also disturbing
/// every other still-pinned tier. Applied as a single [`Edit::Batch`] for the
/// identical "one undo step, one re-solve" reason the all-tiers version is. A
/// silent no-op when the selection is empty or none of it has an
/// `imported_meet` left to adopt.
///
/// Wired to `EditorModel.adopt_selected()` (declared in `ui/models/editor.slint`)
/// -- `editor_tier_table.slint`'s own "Adopt sel." button calls it.
///
/// `pub(super)` for the same reason as [`setup_adopt_all_callback`] above.
pub(super) fn setup_adopt_selected_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_adopt_selected(move || {
        stall_guard("on_adopt_selected", || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // The same batch restricted to the multi-selection --
            // `EditorSession::adopt_selected_imported_meets`.
            match st.adopt_selected_imported_meets() {
                Ok(0) => {}
                Ok(count) => {
                    drop(st);
                    refresh_all_now(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &state,
                        false,
                    );
                    let plural = if count == 1 { "" } else { "s" };
                    show_toast(
                        &ui,
                        &format!("Adopted {count} imported meet{plural} from the selection"),
                        "info",
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
    });
}

/// The inverse of "Adopt" -- freezes a tier's CURRENT solved
/// mast (read back here from the shared `solid_last_solved` cache the tier
/// table's own MAST column is built from) as an exact
/// [`MeetConstraint::ScaleReference`] anchor, through [`Edit::SetConstraint`]
/// exactly like `solve_actions::setup_adopt_meet_callback`'s own "Adopt" --
/// letting a cutter pin a value the solver just derived before optimizing the
/// rest of the design. When there is no current solve for this tier it pins
/// nothing and raises a warning toast ("Wait for the current design to finish
/// solving before pinning."). That covers an out-of-range index, an unpopulated
/// `solid_last_solved`, a cache from an older design generation or a different tier
/// count (the table still shows Pin buttons from the cached rows right after an
/// edit), and a tier whose mast is only an estimate (see [`pinnable_mast`]).
///
/// Does not call `refresh_all`: the value pinned is already what the design's last
/// real solve produced, so it pushes the immediate UI update from the cached solve
/// (`refresh_editor_panel_stale`) and lets the background dirty-set replan
/// (`submit_preview_replan`) confirm it.
///
/// Wired to `EditorModel.pin_to_mast(int)` (declared in `ui/models/editor.slint`)
/// -- the tier table's own per-row "Pin" button calls it.
///
/// `pub(super)` for the same reason as [`setup_adopt_all_callback`] above.
pub(super) fn setup_pin_to_mast_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_pin_to_mast(move |index: i32| {
            stall_guard("on_pin_to_mast", || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                let Ok(index) = usize::try_from(index) else {
                    return;
                };
                let mut st = state.borrow_mut();
                let mast = {
                    let cache = solid_last_solved
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    pinnable_mast(
                        cache.as_ref(),
                        st.current_generation(),
                        st.design.tiers.len(),
                        index,
                    )
                };
                let Some(mast) = mast else {
                    show_toast(
                        &ui,
                        "Wait for the current design to finish solving before pinning.",
                        "warning",
                    );
                    return;
                };
                match st.apply(Edit::SetConstraint {
                    index,
                    constraint: MeetConstraint::ScaleReference(mast),
                }) {
                    Ok(()) => {
                        // Pin freezes a tier at exactly the mast the last real solve
                        // already
                        // produced for it (this function's own doc comment), so a full
                        // `refresh_all` re-solve is redundant work for the SAME reason
                        // `setup_adopt_meet_callback`'s own doc comment gives -- and,
                        // unlike a wholesale replace, this touches only one tier.
                        // Pushes the immediate UI update from the CACHED solve
                        // (`refresh_editor_panel_stale` with `dirty = {index}`, which
                        // reads every OTHER row from the last-solved cache -- see
                        // `push_stale_content`'s own doc comment) and lets the
                        // background dirty-set replan confirm it via `resolve_dirty`
                        // instead of re-solving the whole design synchronously here.
                        let dirty: BTreeSet<usize> = std::iter::once(index).collect();
                        refresh_editor_panel_stale(&ui, &render_ctx, &st, &dirty);
                        submit_preview_replan(
                            &ui,
                            &render_ctx,
                            &preview_state,
                            &solid_last_solved,
                            &st,
                            dirty,
                            false,
                        );
                        show_toast(
                            &ui,
                            &format!("Pinned tier {} at {mast:.4}", index + 1),
                            "info",
                        );
                    }
                    Err(e) => show_toast(&ui, &e.to_string(), "error"),
                }
            });
        });
}

/// The mast of tier `index` from the last real solve, when it is safe to freeze.
///
/// `None` when there is no cache, when the cache belongs to another design `generation`
/// or another tier count (the design was edited since), when `index` is out of range, or
/// when the mast is only an estimate (`LeastSquaresFallback`) or a placeholder (`Failed`).
fn pinnable_mast(
    cache: Option<&(u64, Vec<SolvedTier>)>,
    generation: u64,
    tier_count: usize,
    index: usize,
) -> Option<f64> {
    cache
        .filter(|(cached, solved)| *cached == generation && solved.len() == tier_count)
        .and_then(|(_, solved)| solved.get(index))
        .filter(|tier| {
            !matches!(
                tier.strategy,
                SolveStrategy::LeastSquaresFallback | SolveStrategy::Failed
            )
        })
        .map(|tier| tier.mast)
}

#[cfg(test)]
mod tests {
    use super::pinnable_mast;
    use indicatrix::geometry::meet_solver::{SolveStrategy, SolvedTier};

    fn tier(mast: f64, strategy: SolveStrategy) -> SolvedTier {
        SolvedTier {
            mast,
            strategy,
            detail: String::new(),
        }
    }

    fn cache(generation: u64) -> (u64, Vec<SolvedTier>) {
        (
            generation,
            vec![
                tier(1.5, SolveStrategy::DependencyOrder),
                tier(2.5, SolveStrategy::LeastSquaresFallback),
                tier(3.5, SolveStrategy::Failed),
                tier(4.5, SolveStrategy::ScaleReference),
                tier(5.5, SolveStrategy::JointGroup),
            ],
        )
    }

    #[test]
    fn a_current_solved_tier_pins_its_mast() {
        let cache = cache(7);
        assert_eq!(pinnable_mast(Some(&cache), 7, 5, 0), Some(1.5));
        assert_eq!(pinnable_mast(Some(&cache), 7, 5, 3), Some(4.5));
        assert_eq!(pinnable_mast(Some(&cache), 7, 5, 4), Some(5.5));
    }

    #[test]
    fn a_stale_generation_pins_nothing() {
        let cache = cache(7);
        assert_eq!(pinnable_mast(Some(&cache), 8, 5, 0), None);
    }

    #[test]
    fn a_different_tier_count_pins_nothing() {
        let cache = cache(7);
        assert_eq!(pinnable_mast(Some(&cache), 7, 4, 0), None);
        assert_eq!(pinnable_mast(Some(&cache), 7, 6, 0), None);
    }

    #[test]
    fn estimated_and_failed_masts_are_not_pinned() {
        let cache = cache(7);
        assert_eq!(pinnable_mast(Some(&cache), 7, 5, 1), None);
        assert_eq!(pinnable_mast(Some(&cache), 7, 5, 2), None);
    }

    #[test]
    fn no_cache_or_an_out_of_range_index_pins_nothing() {
        let cache = cache(7);
        assert_eq!(pinnable_mast(None, 7, 5, 0), None);
        assert_eq!(pinnable_mast(Some(&cache), 7, 5, 5), None);
    }
}
