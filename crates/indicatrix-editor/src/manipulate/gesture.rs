//! One handle drag as pure decisions.
//!
//! What a gesture captured at the press, which edit each throttled pointer value asks
//! for (and which it does not, so a still pointer costs no undo churn), the live
//! feedback and the closing toast.
//!
//! The desktop and the web app each own the plumbing around it (the pointer, the timer
//! that throttles values to one per frame, the toast widget); this module is the part
//! that must not differ between them.

use super::{
    dependents::moved_tiers,
    drag::{DragStart, DragValue, SnapMode},
    handles::HandleKind,
    projection::ScreenPoint,
    provisional::Snapshot,
    target::HandleTarget,
    text,
};
use crate::session::EditorSession;
use indicatrix::geometry::meet_solver::{MeetConstraint, SolvedTier};
use indicatrix_cut_core::{ConstraintTier, EditError};
use std::time::Duration;

/// A tier whose solved mast moved by more than this counts as following the drag.
pub const MOVED_TOLERANCE: f64 = 1e-6;

/// The handle `kind` int the UI globals use (`0` angle, `1` depth, `2` index).
#[must_use]
pub const fn kind_from_int(kind: i32) -> Option<HandleKind> {
    match kind {
        0 => Some(HandleKind::Angle),
        1 => Some(HandleKind::Depth),
        2 => Some(HandleKind::Index),
        _ => None,
    }
}

/// The inverse of [`kind_from_int`].
#[must_use]
pub const fn kind_to_int(kind: HandleKind) -> i32 {
    match kind {
        HandleKind::Angle => 0,
        HandleKind::Depth => 1,
        HandleKind::Index => 2,
    }
}

/// How finely a drag snaps: the Snap pill turns snapping off outright, otherwise Shift
/// selects the fine step.
#[must_use]
pub const fn snap_mode(shift: bool, snap_off: bool) -> SnapMode {
    if snap_off {
        SnapMode::Off
    } else if shift {
        SnapMode::Fine
    } else {
        SnapMode::Coarse
    }
}

/// A tier's name for hints and toasts: its own name, else `"tier N"` (1-based, like the
/// tier table's `#` column).
#[must_use]
pub fn tier_label(tier: &ConstraintTier, index: usize) -> String {
    if tier.name.is_empty() {
        format!("tier {}", index + 1)
    } else {
        tier.name.clone()
    }
}

/// One edit a throttled drag value asks for, relative to what the gesture has already
/// applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Set the tier's angle to this many degrees.
    Angle(f64),
    /// Pin the tier's mast at this value.
    Mast(f64),
    /// Turn the tier's index-wheel positions by this many MORE whole teeth.
    Teeth(i64),
}

/// What one applied [`Step`] reported back.
#[derive(Debug, Clone, PartialEq)]
pub struct AppliedEdit {
    /// The session generation after the edit.
    pub generation: u64,
    /// The meet constraint a depth pin replaced (only the gesture's first pin has one).
    pub replaced_meet: Option<MeetConstraint>,
}

/// What a gesture has applied so far. The decision logic ([`drain_value`]) lives on
/// this plain struct so it can be tested without a window.
#[derive(Debug, Default)]
pub struct DragProgress {
    /// The newest value handed to the apply hook (applied or found unchanged).
    pub last_value: Option<DragValue>,
    /// Whole teeth the index handle has turned the tier by since the press.
    pub teeth_applied: i64,
    /// Whether any edit actually reached the design.
    pub applied_any: bool,
    /// The session generation after the newest applied edit.
    pub applied_generation: Option<u64>,
    /// The first meet constraint a depth pin of this gesture replaced.
    pub replaced_meet: Option<MeetConstraint>,
}

impl DragProgress {
    /// The edit `value` asks for, or `None` when it equals the last value handed out
    /// (no redundant undo churn) or, for the index handle, asks for no further turn.
    /// Index values are running totals from the press, so the step is the difference to
    /// what is already applied.
    pub fn plan(&mut self, value: DragValue) -> Option<Step> {
        if self.last_value == Some(value) {
            return None;
        }
        self.last_value = Some(value);
        match value {
            DragValue::AngleDeg(deg) => Some(Step::Angle(deg)),
            DragValue::Mast(mast) => Some(Step::Mast(mast)),
            DragValue::IndexTeeth(total) => {
                let more = total - self.teeth_applied;
                (more != 0).then_some(Step::Teeth(more))
            }
        }
    }

    /// Records that `step` reached the design.
    pub fn record(&mut self, step: Step, applied: AppliedEdit) {
        self.applied_any = true;
        self.applied_generation = Some(applied.generation);
        if let Step::Teeth(more) = step {
            self.teeth_applied += more;
        }
        if self.replaced_meet.is_none() {
            self.replaced_meet = applied.replaced_meet;
        }
    }
}

/// The whole per-value decision of a drain tick.
///
/// Plans the edit `value` asks for, hands it to `apply` once, and records what came
/// back. Returns whether an edit reached the design. `apply` returns `None` for
/// "nothing changed" and after reporting an error.
pub fn drain_value(
    progress: &mut DragProgress,
    value: DragValue,
    apply: impl FnOnce(Step) -> Option<AppliedEdit>,
) -> bool {
    let Some(step) = progress.plan(value) else {
        return false;
    };
    let Some(applied) = apply(step) else {
        return false;
    };
    progress.record(step, applied);
    true
}

/// Performs one planned [`Step`] on `tier` of `session` as a coalescing edit stamped
/// `now`. `Ok(None)` when nothing changed.
///
/// # Errors
///
/// The session's own edit error.
pub fn apply_step(
    session: &mut EditorSession,
    tier: usize,
    step: Step,
    now: Duration,
) -> Result<Option<AppliedEdit>, EditError> {
    Ok(match step {
        Step::Angle(deg) => session
            .set_tier_angle(tier, deg, now)?
            .map(|change| AppliedEdit {
                generation: change.generation,
                replaced_meet: None,
            }),
        Step::Mast(mast) => session
            .pin_tier_mast(tier, mast, now)?
            .map(|pin| AppliedEdit {
                generation: pin.change.generation,
                replaced_meet: pin.replaced_meet,
            }),
        Step::Teeth(more) => {
            session
                .rotate_tier_indices(tier, more, now)?
                .map(|change| AppliedEdit {
                    generation: change.generation,
                    replaced_meet: None,
                })
        }
    })
}

/// The value a handle has before any pointer travel.
#[must_use]
pub const fn start_value(kind: HandleKind, start_angle_deg: f64, start_mast: f64) -> DragValue {
    match kind {
        HandleKind::Angle => DragValue::AngleDeg(start_angle_deg),
        HandleKind::Depth => DragValue::Mast(start_mast),
        HandleKind::Index => DragValue::IndexTeeth(0),
    }
}

/// What a press on a handle captures from the design it will edit.
#[derive(Debug, Clone)]
pub struct GestureInputs {
    /// The tier's angle now.
    pub start_angle_deg: f64,
    /// The solved masts, when they describe the design one to one.
    pub masts: Option<Vec<SolvedTier>>,
    /// The provisional tier to restore on Escape (`None` for a committed tier, whose
    /// gesture is undone through the history).
    pub restore: Option<Snapshot>,
}

/// A gesture in progress.
#[derive(Debug)]
pub struct ActiveDrag {
    /// What the press captured (handle, start values, pointer, layout).
    pub start: DragStart,
    /// The tier being dragged.
    pub tier: usize,
    /// The tier's name for hints and toasts.
    pub label: String,
    /// The solved masts at the press, to tell which tiers follow the drag.
    pub start_masts: Vec<SolvedTier>,
    /// What the gesture has applied so far.
    pub progress: DragProgress,
    /// The newest value the pointer asks for, applied or not yet (the release flushes it).
    pub requested: Option<DragValue>,
    /// The tiers currently outlined as following the drag.
    pub outlined: Vec<usize>,
    /// The clock every edit of the gesture is stamped with, so a pointer held still for
    /// longer than the coalescing window cannot split the drag into two undo steps.
    pub gesture_now: Duration,
    /// Whether the gesture edits the provisional slice tier (a clone kept by the Slice
    /// tool, no history entry, no toast) instead of the session.
    pub provisional: bool,
    /// The provisional tier as it was at the press, for Escape.
    pub restore: Option<Snapshot>,
}

impl ActiveDrag {
    /// The gesture a press on `kind`'s handle of `target` starts, or `None` when a depth
    /// drag has no solved masts to start from (the caller says
    /// [`text::NEEDS_SOLVE_HINT`]).
    ///
    /// An unsolved design has no masts: the mast falls back to 0, which the angle and
    /// index drags never read.
    #[must_use]
    pub fn begin(
        kind: HandleKind,
        target: &HandleTarget,
        inputs: GestureInputs,
        pointer: ScreenPoint,
        gesture_now: Duration,
    ) -> Option<Self> {
        let GestureInputs {
            start_angle_deg,
            masts,
            restore,
        } = inputs;
        if kind == HandleKind::Depth && masts.is_none() {
            return None;
        }
        let start_masts = masts.unwrap_or_default();
        let start_mast = start_masts.get(target.tier).map_or(0.0, |s| s.mast);
        Some(Self {
            start: DragStart {
                kind,
                start_angle_deg,
                start_mast,
                pointer,
                layout: target.layout,
            },
            tier: target.tier,
            label: target.label.clone(),
            start_masts,
            progress: DragProgress::default(),
            requested: None,
            outlined: Vec::new(),
            gesture_now,
            provisional: target.provisional,
            restore,
        })
    }

    /// The hint line right after the press: the provisional tier's own hint, else the
    /// live hint at the starting value.
    #[must_use]
    pub fn opening_hint(&self, provisional_hint: Option<String>) -> String {
        if self.provisional {
            return provisional_hint.unwrap_or_default();
        }
        let value = start_value(
            self.start.kind,
            self.start.start_angle_deg,
            self.start.start_mast,
        );
        text::drag_live_hint(self.start.kind, &self.label, &value, 0)
    }

    /// The value the hint line shows: the newest the pointer asked for.
    #[must_use]
    pub fn live_value(&self) -> DragValue {
        let untouched = start_value(
            self.start.kind,
            self.start.start_angle_deg,
            self.start.start_mast,
        );
        self.requested
            .or(self.progress.last_value)
            .unwrap_or(untouched)
    }

    /// The value the toast reports: the index handle's running turn, else the last value
    /// the gesture handed to the design.
    #[must_use]
    pub fn final_value(&self) -> DragValue {
        match self.start.kind {
            HandleKind::Index => DragValue::IndexTeeth(self.progress.teeth_applied),
            _ => self.live_value(),
        }
    }

    /// The live hint ("P1 -> 41.3 deg, 3 other tiers follow") and the tiers whose solved
    /// mast has moved since the press, given the masts `now_masts` of the newest frame
    /// (`None` when they do not describe the design).
    #[must_use]
    pub fn live_feedback(&self, now_masts: Option<&[SolvedTier]>) -> (String, Vec<usize>) {
        let moved = now_masts.map_or_else(Vec::new, |now| {
            moved_tiers(&self.start_masts, now, self.tier, MOVED_TOLERANCE)
        });
        let hint = text::drag_live_hint(
            self.start.kind,
            &self.label,
            &self.live_value(),
            moved.len(),
        );
        (hint, moved)
    }

    /// The toast after the gesture ends: what was set and that Undo restores it. `None`
    /// for a gesture that changed nothing, and for a provisional tier (its hint line
    /// and Keep button are the feedback).
    #[must_use]
    pub fn done_toast(&self) -> Option<String> {
        (self.progress.applied_any && !self.provisional).then(|| {
            text::drag_done_toast(
                self.start.kind,
                &self.label,
                &self.final_value(),
                self.progress.replaced_meet.as_ref(),
            )
        })
    }
}
