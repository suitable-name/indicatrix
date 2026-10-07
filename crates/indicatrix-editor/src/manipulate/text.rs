//! Every user-facing string of the direct-manipulation tools.
//!
//! Living here lets the desktop and the web app word them identically: handle hover
//! hints, the live drag hint, the toast after a drag, and the slice tool's hints and
//! toasts.

use super::{
    drag::{DragValue, SnapMode},
    handles::HandleKind,
};
use indicatrix::geometry::meet_solver::MeetConstraint;

/// The toast after an Escape that restored the design.
pub const DRAG_CANCELLED_TOAST: &str = "Drag cancelled. The design is back the way it was.";

/// The toast when Escape found the design changed by something else since the drag.
pub const DRAG_CANCEL_SKIPPED_TOAST: &str = "Something else changed the design during the drag, so Escape left it alone. Use Undo to step back.";

/// The hint when a depth drag cannot start.
pub const NEEDS_SOLVE_HINT: &str =
    "Solve the design first: a depth drag needs the tier's solved mast.";

/// The hint after a line too short to define a plane.
pub const SLICE_TOO_SHORT_HINT: &str =
    "That line is too short to cut a facet. Drag a longer line across the stone.";

/// The hint when there is no picture of the stone yet.
pub const SLICE_NEEDS_PICTURE_HINT: &str =
    "Nothing to slice yet: solve the design first so the stone is on screen.";

/// The toast when an edit of the committed design ended the Slice session.
pub const SLICE_CHANGED_TOAST: &str = "Slice discarded -- the design changed";

/// The toast when a tier-list selection ended the Slice session.
pub const SLICE_SELECTION_TOAST: &str = "Slice discarded -- another tier was selected";

/// What the hint line and the refused Keep say while the provisional tier cuts nothing.
///
/// A fresh slice sits exactly at the tangency mast, so it has no facet on the stone
/// until its depth handle is dragged inward.
pub const SLICE_NO_DEPTH_HINT: &str =
    "Drag the depth handle inward first -- the facet does not touch the stone yet";

/// The hint while the Cut slider hides the provisional tier (the mesh then holds only the
/// first tiers' planes).
pub const SLICE_CUT_SLIDER_HINT: &str =
    "The Cut slider is hiding the new facet: move it to the end to see and adjust it.";

/// The toast when Escape could not undo a cancelled drag.
#[must_use]
pub fn drag_cancel_failed_toast(error: &str) -> String {
    format!("Could not cancel the drag: {error}")
}

/// `value` with at most `max_decimals` decimals, trailing zeros trimmed but one decimal
/// kept: `41.3`, `41.0`, `0.673`.
fn trimmed(value: f64, max_decimals: usize) -> String {
    let text = format!("{value:.max_decimals$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').to_string()
    } else {
        text
    };
    if text.ends_with('.') {
        format!("{text}0")
    } else {
        text
    }
}

/// "1 facet" / "3 facets".
fn facets(count: usize) -> String {
    if count == 1 {
        "1 facet".to_string()
    } else {
        format!("{count} facets")
    }
}

/// "1 tooth" / "3 teeth".
fn teeth(count: u64) -> String {
    if count == 1 {
        "1 tooth".to_string()
    } else {
        format!("{count} teeth")
    }
}

/// "3 teeth forward" / "2 teeth back" (`k` is the signed number of teeth).
fn teeth_phrase(k: i64) -> String {
    if k >= 0 {
        format!("{} forward", teeth(k.unsigned_abs()))
    } else {
        format!("{} back", teeth(k.unsigned_abs()))
    }
}

/// What the current snapping does to `kind`'s value, as a short sentence.
const fn snap_clause(kind: HandleKind, snap: SnapMode) -> &'static str {
    match (kind, snap) {
        (HandleKind::Index, _) => "Moves in whole teeth.",
        (HandleKind::Angle, SnapMode::Coarse) => "Snaps to 0.1 deg; hold Shift for 0.01.",
        (HandleKind::Angle, SnapMode::Fine) => "Snaps to 0.01 deg.",
        (HandleKind::Depth, SnapMode::Coarse) => "Snaps to 0.01; hold Shift for 0.001.",
        (HandleKind::Depth, SnapMode::Fine) => "Snaps to 0.001.",
        (HandleKind::Angle | HandleKind::Depth, SnapMode::Off) => "Snapping is off.",
    }
}

/// The hint line while the pointer rests on `kind`'s handle of `tier_label`, before any
/// drag: what dragging does, how it snaps, and how many tiers meet this tier by name
/// (and so will follow it).
#[must_use]
pub fn handle_hover_hint(
    kind: HandleKind,
    tier_label: &str,
    meeting_count: usize,
    snap: SnapMode,
) -> String {
    let action = match kind {
        HandleKind::Angle => format!("Drag to tilt {tier_label} (every facet of the tier)."),
        HandleKind::Depth => format!("Drag to move {tier_label} in or out (its mast)."),
        HandleKind::Index => format!("Drag to turn {tier_label} around the index wheel."),
    };
    let follow = match meeting_count {
        0 => String::new(),
        1 => " 1 tier meets it by name and follows.".to_string(),
        n => format!(" {n} tiers meet it by name and follow."),
    };
    format!("{action} {}{follow}", snap_clause(kind, snap))
}

/// The hint line right after `tier_label`'s facet is selected, for screens with no hover.
///
/// Meant for touch and pen: it says which of the three handles does what, by the letters
/// the handles carry on screen (A angle, D depth, I index).
#[must_use]
pub fn handle_select_hint(tier_label: &str) -> String {
    format!(
        "{tier_label} selected. Drag A to tilt it, D to move it in or out, or I to turn it around the index wheel."
    )
}

/// The hint line while a tier whose angle follows a relation is selected: why it has no
/// angle handle, and what to do instead. `relation` is the relation as a cutter reads it
/// (`"P1 - 2"`).
#[must_use]
pub fn angle_follows_relation_hint(tier_label: &str, relation: &str) -> String {
    format!(
        "{tier_label} follows a relation ({tier_label} = {relation}), so it has no angle handle. \
         Tilt the tier it follows, or edit the relation in the Tier form."
    )
}

/// The hint line while a drag is in progress: what the tier will become, and how many
/// other tiers follow it, e.g. `"P1 -> 41.3 deg, 3 other tiers follow"`.
///
/// A `kind` and `value` that do not belong together (a caller bug) render as the bare
/// tier label.
#[must_use]
pub fn drag_live_hint(
    kind: HandleKind,
    tier_label: &str,
    value: &DragValue,
    followers: usize,
) -> String {
    let head = match (kind, value) {
        (HandleKind::Angle, DragValue::AngleDeg(deg)) => {
            // The angle a person reads is a magnitude; the side of the girdle is the tier's.
            format!("{tier_label} -> {} deg", trimmed(deg.abs(), 3))
        }
        (HandleKind::Depth, DragValue::Mast(mast)) => {
            format!("{tier_label} -> mast {}", trimmed(*mast, 3))
        }
        (HandleKind::Index, DragValue::IndexTeeth(k)) => {
            format!("{tier_label} -> {}", teeth_phrase(*k))
        }
        _ => tier_label.to_string(),
    };
    match followers {
        0 => head,
        1 => format!("{head}, 1 other tier follows"),
        n => format!("{head}, {n} other tiers follow"),
    }
}

/// What a depth drag pinned the mast over, as a sentence for the toast.
fn replaced_clause(replaced: &MeetConstraint) -> String {
    match replaced {
        MeetConstraint::MeetNamed(names) if !names.is_empty() => {
            format!(" It met {} before; Undo restores that.", names.join(", "))
        }
        MeetConstraint::MeetNamed(_) | MeetConstraint::MeetExisting => {
            " It met the vertex the solver found before; Undo restores that.".to_string()
        }
        MeetConstraint::ScaleReference(mast) => format!(
            " It was pinned at mast {} before; Undo restores that.",
            trimmed(*mast, 3)
        ),
    }
}

/// The toast after a drag gesture ends: what was set, and that Undo restores it. For a
/// depth drag that replaced a meet constraint (`replaced_meet`), it also says what the
/// tier used to meet.
#[must_use]
pub fn drag_done_toast(
    kind: HandleKind,
    tier_label: &str,
    value: &DragValue,
    replaced_meet: Option<&MeetConstraint>,
) -> String {
    let done = match value {
        DragValue::AngleDeg(deg) => format!(
            "Set {tier_label} to {} deg. Undo restores the old angle.",
            trimmed(deg.abs(), 3)
        ),
        DragValue::Mast(mast) => format!(
            "Pinned {tier_label} at mast {}. Undo restores the old depth.",
            trimmed(*mast, 3)
        ),
        DragValue::IndexTeeth(0) => format!("{tier_label} stays where it was."),
        DragValue::IndexTeeth(k) => format!(
            "Turned {tier_label} {}. Undo turns it back.",
            teeth_phrase(*k)
        ),
    };
    match replaced_meet {
        Some(replaced) if kind == HandleKind::Depth => {
            format!("{done}{}", replaced_clause(replaced))
        }
        _ => done,
    }
}

/// The hint line while the Slice tool waits for a drag.
#[must_use]
pub fn slice_mode_hint(symmetric: bool) -> String {
    let orbit = if symmetric {
        "The new facet is repeated around the whole symmetric set."
    } else {
        "The new facet is a single index."
    };
    format!(
        "Slice: drag a line across the stone; the part to the right of your drag is cut away. {orbit} Esc leaves Slice."
    )
}

/// The hint line while a provisional slice tier is on screen: what it is, and how to
/// keep or drop it.
#[must_use]
pub fn slice_provisional_hint(
    tier_label: &str,
    facet_count: usize,
    angle_deg: f64,
    index: f64,
) -> String {
    format!(
        "New tier {tier_label}: {} at {} deg, index {}. Drag its handles to adjust; Enter keeps it, Esc discards it.",
        facets(facet_count),
        trimmed(angle_deg.abs(), 3),
        trimmed(index, 2)
    )
}

/// The toast after Keep commits a provisional slice tier as one undo step.
#[must_use]
pub fn slice_kept_toast(tier_label: &str, facet_count: usize, angle_deg: f64) -> String {
    format!(
        "Kept {tier_label}: {} at {} deg. Undo removes it.",
        facets(facet_count),
        trimmed(angle_deg.abs(), 3)
    )
}

/// The toast after Discard drops a provisional slice tier.
#[must_use]
pub fn slice_discarded_toast(tier_label: &str) -> String {
    format!("Discarded {tier_label}. Nothing was changed.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relation_hint_names_the_tier_its_relation_and_the_way_out() {
        let hint = angle_follows_relation_hint("P2", "P1 - 2");
        assert_eq!(
            hint,
            "P2 follows a relation (P2 = P1 - 2), so it has no angle handle. \
             Tilt the tier it follows, or edit the relation in the Tier form."
        );
    }
}
