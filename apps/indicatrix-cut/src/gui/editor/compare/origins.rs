//! Gathering each origin's two sides on the UI thread, and the "is this still what
//! you compared?" check "Keep after" runs before handing over to the originating
//! feature's own Apply handler.

use super::session::{CompareOrigin, SideInput, resolve_metrics_material, resolve_side_material};
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
    /// A comparison of saved variants -- nothing to keep.
    Variants,
}

/// One side of a variants comparison, handed over by the Variants view.
pub(in crate::gui::editor) struct VariantSide {
    /// The design to show.
    pub(in crate::gui::editor) design: Design,
    /// The words over its picture.
    pub(in crate::gui::editor) label: String,
}

/// What the Optimize comparison's before side is measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Reference {
    /// The live design (today's behaviour, the default).
    #[default]
    Current,
    /// The original held before the last Retarget Apply.
    Original,
}

impl Reference {
    /// The picker's entry index.
    pub(super) const fn index(self) -> i32 {
        match self {
            Self::Current => 0,
            Self::Original => 1,
        }
    }

    /// The reference a picker index names (anything but 1 is the current design).
    pub(super) const fn from_index(index: i32) -> Self {
        if index == 1 {
            Self::Original
        } else {
            Self::Current
        }
    }
}

/// The "Compare against" picker's state for one request: which entry is chosen, and the
/// original's label when the picker is offered at all (`None` hides it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ReferenceOffer {
    /// The chosen entry.
    pub(super) selected: Reference,
    /// The held original's label; `Some` only for Optimize with an original held.
    pub(super) original_label: Option<String>,
}

/// The picker's entries: always "Current design", plus the original when one is held.
#[must_use]
pub(super) fn reference_options(offer: &ReferenceOffer) -> Vec<String> {
    let mut options = vec!["Current design".to_string()];
    if let Some(label) = &offer.original_label {
        options.push(format!("Original ({label})"));
    }
    options
}

/// Everything `super::wiring::open_compare` needs to open a session.
pub(super) struct OpenRequest {
    /// The "Compare against" picker (hidden for every origin but Optimize).
    pub(super) reference: ReferenceOffer,
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
    let metrics_material = resolve_metrics_material(&design, custom);
    SideInput {
        design,
        label,
        material,
        metrics_material,
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
        reference: ReferenceOffer::default(),
        origin: CompareOrigin::Retarget,
        guard: KeepGuard::Retarget(Box::new(inputs.guard)),
        before: side(inputs.current, "Current design".to_string(), &custom),
        after: side(inputs.candidate, inputs.after_label, &custom),
    })
}

/// The before side's design and words, and the picker state: the live design for
/// [`Reference::Current`], the held original for [`Reference::Original`] -- which falls
/// back to the live design when no original is held (`original` is `None`). The picker is
/// offered iff an original is held.
pub(super) fn pick_before(
    current: &Design,
    original: Option<(Design, String)>,
    wanted: Reference,
) -> (Design, String, ReferenceOffer) {
    match original {
        Some((design, label)) => {
            let offer = ReferenceOffer {
                selected: wanted,
                original_label: Some(label.clone()),
            };
            match wanted {
                Reference::Original => (design, format!("Original \"{label}\""), offer),
                Reference::Current => (current.clone(), "Current design".to_string(), offer),
            }
        }
        None => (
            current.clone(),
            "Current design".to_string(),
            ReferenceOffer::default(),
        ),
    }
}

/// Optimize: before = the live design (or, when picked and held, the original kept by the
/// last Retarget Apply), after = `build_optimize_preview_design` of the held outcome --
/// the same candidate the Optimize tab's own Preview shows.
///
/// # Errors
///
/// When no outcome is held, or the design moved on since the search ran (Apply
/// would refuse it for the same reason).
pub(super) fn optimize_request(
    st: &EditorState,
    render_ctx: &Arc<Mutex<RenderContext>>,
    reference: Reference,
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
    let (before_design, before_label, offer) =
        pick_before(&st.design, callbacks::original_for_compare(), reference);
    Ok(OpenRequest {
        reference: offer,
        origin: CompareOrigin::Optimize,
        guard: KeepGuard::Optimize {
            outcome: Box::new(outcome),
            generation,
        },
        before: side(before_design, before_label, &custom),
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
        reference: ReferenceOffer::default(),
        origin: CompareOrigin::Snapshot,
        guard: KeepGuard::Snapshot,
        before: side(snapshot, format!("Snapshot \"{label}\""), &custom),
        after: side(st.design.clone(), "Current design".to_string(), &custom),
    })
}

/// Variants: `first` on the before side and `second` on the after side. View only.
pub(super) fn variants_request(
    render_ctx: &Arc<Mutex<RenderContext>>,
    first: VariantSide,
    second: VariantSide,
) -> OpenRequest {
    let custom = custom_materials(render_ctx);
    OpenRequest {
        reference: ReferenceOffer::default(),
        origin: CompareOrigin::Variants,
        guard: KeepGuard::Variants,
        before: side(first.design, first.label, &custom),
        after: side(second.design, second.label, &custom),
    }
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
        KeepGuard::Snapshot | KeepGuard::Variants => false,
    }
}
