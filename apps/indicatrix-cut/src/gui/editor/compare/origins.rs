//! Gathering each origin's two sides on the UI thread, and the "is this still what
//! you compared?" check "Keep after" runs before handing over to the originating
//! feature's own Apply handler.

use super::session::{CompareOrigin, SideInput, resolve_side_material};
use crate::{
    MainWindow,
    bridge::render_thread::RenderContext,
    gui::editor::{
        callbacks::{self, RetargetCompareInputs, RetargetKeepGuard},
        state::EditorState,
        view::build_optimize_preview_design,
    },
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{Design, OptimizeOutcome};
use std::sync::{Arc, Mutex, PoisonError, atomic::Ordering as AtomicOrdering};

/// What "Keep after" would commit, captured when the window opened -- compared
/// against the originating feature's CURRENT pending result by
/// [`guard_still_matches`] before Keep hands over to that feature's own Apply.
/// Holds data only; nothing here can apply an edit. Boxed payloads: a proposal or
/// outcome is large next to the empty `Snapshot` variant.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum KeepGuard {
    /// Retarget's held proposal, its generation and its target material.
    Retarget(Box<RetargetKeepGuard>),
    /// Optimize's held outcome and the generation it ran against.
    Optimize {
        /// The pending outcome.
        outcome: Box<OptimizeOutcome>,
        /// The design generation the search ran against.
        generation: u64,
    },
    /// A snapshot comparison -- nothing to keep.
    Snapshot,
}

/// Everything `super::wiring::open_compare` needs to open a session.
pub(super) struct OpenRequest {
    /// Which feature asked.
    pub(super) origin: CompareOrigin,
    /// What Keep would commit.
    pub(super) guard: KeepGuard,
    /// The "before" side.
    pub(super) before: SideInput,
    /// The "after" side.
    pub(super) after: SideInput,
}

/// The custom-catalogue materials the live viewport resolves against.
fn custom_materials(render_ctx: &Arc<Mutex<RenderContext>>) -> Vec<GemMaterial> {
    render_ctx
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone()
}

/// One side's inputs, its traced material resolved against `custom`.
fn side(design: Design, label: String, custom: &[GemMaterial]) -> SideInput {
    let material = resolve_side_material(&design, custom);
    SideInput {
        design,
        label,
        material,
    }
}

/// Retarget: before = the live design, after = the held proposal's candidate in
/// the dialog's target material.
///
/// # Errors
///
/// [`callbacks::retarget_compare_inputs`]'s cutter-facing reason.
pub(super) fn retarget_request(
    ui: &MainWindow,
    st: &EditorState,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Result<OpenRequest, String> {
    let inputs: RetargetCompareInputs = callbacks::retarget_compare_inputs(ui, st, render_ctx)?;
    let custom = custom_materials(render_ctx);
    Ok(OpenRequest {
        origin: CompareOrigin::Retarget,
        guard: KeepGuard::Retarget(Box::new(inputs.guard)),
        before: side(inputs.current, "Current design".to_string(), &custom),
        after: side(inputs.candidate, inputs.after_label, &custom),
    })
}

/// Optimize: before = the live design, after = `build_optimize_preview_design` of
/// the held outcome -- the same candidate the Optimize tab's own Preview shows.
///
/// # Errors
///
/// When no outcome is held, or the design moved on since the search ran (Apply
/// would refuse it for the same reason).
pub(super) fn optimize_request(
    st: &EditorState,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Result<OpenRequest, String> {
    let pending = st
        .pending_optimize
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let (outcome, generation) =
        pending.ok_or_else(|| "No Optimize result to compare yet.".to_string())?;
    if generation != st.generation.load(AtomicOrdering::Relaxed) {
        return Err("The design changed since Optimize ran -- re-run Optimize first.".to_string());
    }
    let candidate = build_optimize_preview_design(&st.design, &outcome);
    let custom = custom_materials(render_ctx);
    Ok(OpenRequest {
        origin: CompareOrigin::Optimize,
        guard: KeepGuard::Optimize {
            outcome: Box::new(outcome),
            generation,
        },
        before: side(st.design.clone(), "Current design".to_string(), &custom),
        after: side(candidate, "Optimize candidate".to_string(), &custom),
    })
}

/// Snapshot: before = the held snapshot, after = the live design. View only.
///
/// # Errors
///
/// When no snapshot has been taken this session.
pub(super) fn snapshot_request(
    st: &EditorState,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Result<OpenRequest, String> {
    let (snapshot, label) = callbacks::snapshot_for_compare()
        .ok_or_else(|| "No snapshot taken yet -- use Snapshot Design first.".to_string())?;
    let custom = custom_materials(render_ctx);
    Ok(OpenRequest {
        origin: CompareOrigin::Snapshot,
        guard: KeepGuard::Snapshot,
        before: side(snapshot, format!("Snapshot \"{label}\""), &custom),
        after: side(st.design.clone(), "Current design".to_string(), &custom),
    })
}

/// Whether the originating feature's Apply would still commit exactly what the
/// window compared -- `false` once the proposal/outcome was rebuilt, consumed,
/// dropped or went stale while the window was open (and always for a snapshot).
#[must_use]
pub(super) fn guard_still_matches(
    guard: &KeepGuard,
    ui: &MainWindow,
    st: &EditorState,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> bool {
    match guard {
        KeepGuard::Retarget(expected) => {
            callbacks::current_retarget_guard(ui, st, render_ctx).as_ref() == Some(&**expected)
        }
        KeepGuard::Optimize {
            outcome,
            generation,
        } => {
            let pending = st
                .pending_optimize
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            pending.is_some_and(|(held, held_generation)| {
                held == **outcome && held_generation == *generation
            })
        }
        KeepGuard::Snapshot => false,
    }
}
