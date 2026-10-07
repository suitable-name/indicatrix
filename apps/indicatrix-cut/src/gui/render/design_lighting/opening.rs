//! What opening a design should do: what the library holds for it ([`Lookup`]), what to do
//! about that ([`decide`]) and what to tell the cutter. Pure functions, tested without a window.

use super::stored::LightingValues;
use std::collections::HashSet;

/// What the library holds for a design, as far as this build can use it.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Lookup {
    /// No lighting is stored, or no design is open.
    NoRow,
    /// Usable lighting.
    Row(LightingValues),
    /// The row names a lighting rig this build does not have.
    UnknownRig(String),
    /// The row (or the library) could not be read.
    Unreadable(String),
}

impl Lookup {
    /// Whether a row exists for the design, usable or not.
    pub(super) const fn has_row(&self) -> bool {
        !matches!(self, Self::NoRow)
    }

    /// The line the settings dialog shows under the heading.
    pub(super) fn summary_text(&self) -> String {
        match self {
            Self::NoRow => String::new(),
            Self::Row(values) => values.summary(),
            Self::UnknownRig(rig) => format!(
                "The saved lighting uses '{rig}', which this version does not have, \
                 so your normal lighting is used."
            ),
            Self::Unreadable(reason) => {
                format!("The saved lighting could not be read ({reason}).")
            }
        }
    }
}

/// Reads a stored row's JSON into a [`Lookup`]: the pure half of [`read_lookup`].
pub(super) fn lookup_from_row(settings_json: Option<&str>) -> Lookup {
    let Some(text) = settings_json else {
        return Lookup::NoRow;
    };
    match LightingValues::from_json(text) {
        Err(reason) => Lookup::Unreadable(reason),
        Ok(values) if values.rig().is_none() => Lookup::UnknownRig(values.lighting_rig),
        Ok(values) => Lookup::Row(values),
    }
}

/// What to do to the live view when a design opens.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Plan {
    /// Leave the lighting as it is.
    Keep,
    /// Show this lighting.
    Apply(LightingValues),
    /// Put the app's normal lighting back.
    Restore,
}

/// What to tell the cutter after a design opened.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Note {
    /// The design's saved lighting is now showing.
    UsingSaved,
    /// The design has none, so the normal lighting is back.
    BackToNormal,
    /// The saved lighting names a rig this version does not have.
    UnknownRig(String),
    /// The saved lighting could not be read.
    Unreadable(String),
}

/// A plan and the note that goes with it.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Decision {
    pub(super) plan: Plan,
    pub(super) note: Option<Note>,
}

/// Decides what opening a design does, given what the library holds for it and whether a
/// design's own lighting is showing now (`override_active`).
///
/// - Usable saved lighting is applied.
/// - No saved lighting restores the normal lighting if a design's own is showing, and
///   changes nothing otherwise.
/// - Saved lighting that cannot be used (an unknown rig, an unreadable row) falls back to
///   the normal lighting the same way, and says so.
pub(super) fn decide(lookup: &Lookup, override_active: bool) -> Decision {
    let back = if override_active {
        Plan::Restore
    } else {
        Plan::Keep
    };
    match lookup {
        Lookup::NoRow => Decision {
            note: override_active.then_some(Note::BackToNormal),
            plan: back,
        },
        Lookup::Row(values) => Decision {
            plan: Plan::Apply(values.clone()),
            note: Some(Note::UsingSaved),
        },
        Lookup::UnknownRig(rig) => Decision {
            plan: back,
            note: Some(Note::UnknownRig(rig.clone())),
        },
        Lookup::Unreadable(reason) => Decision {
            plan: back,
            note: Some(Note::Unreadable(reason.clone())),
        },
    }
}

/// The toast text and kind for `note`.
pub(super) fn note_text(note: &Note) -> (String, &'static str) {
    match note {
        Note::UsingSaved => ("Using this design's saved lighting.".to_string(), "info"),
        Note::BackToNormal => ("Back to your normal lighting.".to_string(), "info"),
        Note::UnknownRig(rig) => (
            format!(
                "This design's saved lighting uses '{rig}', which this version does not \
                 have. Your normal lighting is used instead."
            ),
            "warning",
        ),
        Note::Unreadable(reason) => (
            format!(
                "This design's saved lighting could not be read ({reason}). Your normal \
                 lighting is used instead."
            ),
            "warning",
        ),
    }
}

/// Whether a problem note is new for `design_uuid` in this session, and records it if so:
/// a design that cannot use its saved lighting says so once, not on every reopen.
pub(super) fn first_time_for(told: &mut HashSet<String>, design_uuid: &str) -> bool {
    told.insert(design_uuid.to_string())
}

/// Whether a note may replace the toast on screen. A warning or an error stays up until
/// the cutter dismisses it, and a note must not push one off the screen.
pub(super) fn may_show_over(toast_visible: bool, toast_kind: &str) -> bool {
    !(toast_visible && matches!(toast_kind, "warning" | "error"))
}
