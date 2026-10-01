//! The guided walkthrough's tab-session state.
//!
//! The walkthrough itself (its ten steps, the control groups each step leaves usable and
//! the goals that complete a step) is `indicatrix_editor::guide`, shared with the
//! desktop. What the browser adds is that a reload of the tab must not lose the reader's
//! place: [`GuideProgress`] is the little that survives -- whether the panel is open,
//! which step it is on, whether it is collapsed and where the floating panel was dragged
//! to -- stored as JSON under `sessionStorage["indicatrix.guide.v1"]`.
//!
//! Only the position is stored, never a "done" moment: a step is judged from the design
//! (`indicatrix_editor::guide::reached_completion`), and the restored design decides
//! again whether the restored step is already complete.

use indicatrix_editor::guide::STEPS;
use serde::{Deserialize, Serialize};

/// The `sessionStorage` key the page stores [`GuideProgress`] under.
pub const GUIDE_KEY: &str = "indicatrix.guide.v1";

/// Where the reader is in the walkthrough.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuideProgress {
    /// The panel is showing (and its locks are in force).
    pub open: bool,
    /// The current step, 0-based into `indicatrix_editor::guide::STEPS`.
    pub step: usize,
    /// The panel is collapsed to its "Guide - Step N of M" pill.
    pub collapsed: bool,
    /// The floating panel was dragged; [`Self::float_x`] and [`Self::float_y`] hold the spot.
    pub float_placed: bool,
    /// The dragged panel's left edge, logical pixels.
    pub float_x: f32,
    /// The dragged panel's top edge, logical pixels.
    pub float_y: f32,
}

impl GuideProgress {
    /// Parses a stored value and repairs it: a step past the end is the last step, and a
    /// position that is not a finite, non-negative pair counts as never dragged.
    ///
    /// # Errors
    ///
    /// The JSON parser's message (the caller discards the stored entry).
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str::<Self>(text)
            .map(Self::sanitized)
            .map_err(|e| e.to_string())
    }

    /// The JSON to store.
    ///
    /// # Errors
    ///
    /// The serializer's message (in practice unreachable for these types).
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| e.to_string())
    }

    /// `self` with every field in range.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.step = self.step.min(STEPS.len().saturating_sub(1));
        let sane = |v: f32| v.is_finite() && v >= 0.0;
        if !(sane(self.float_x) && sane(self.float_y)) {
            self.float_placed = false;
            self.float_x = 0.0;
            self.float_y = 0.0;
        }
        self
    }
}

/// The walkthrough lines reworded for the browser: `(desktop line, web line)`.
///
/// They are the lines of the shared walkthrough that name a desktop-only control or place.
/// The shared `indicatrix_editor::guide::STEPS` stay the single source of the steps, their
/// locks and their goals; only these sentences differ. A test pins that every desktop
/// line still exists, so rewording one in the shared crate cannot silently drop its
/// override.
pub const WEB_WORDING: &[(&str, &str)] = &[
    (
        "Click New Design... on the command bar (or File > New Design...).",
        "Click New Design... on the start page (or File > New Design..., or Alt+N).",
    ),
    (
        "Read the status strip at the bottom of the Edit tab.",
        "Read the status strip at the bottom of the window.",
    ),
    (
        "Switch to Live Render to see the stone rendered in Diamond.",
        "Switch to the Render tab to see the stone rendered in Diamond.",
    ),
    (
        "Save it with Save Native, or keep editing.",
        "Save it with File > Save native pair (Ctrl+S), or keep editing.",
    ),
    (
        "Continue with Chapter 8 of the manual: Deep Solve, Optimize, Adopt and Apply.",
        "Try Design > Optimize angles and Retarget for material, or press ? for the keyboard shortcuts.",
    ),
    (
        "Every control is unlocked again. Reopen this guide any time from Help > Guide: New Design Walkthrough.",
        "Every control is unlocked again. Reopen this guide any time from Help > Guided walkthrough.",
    ),
];

/// `desktop` as the browser build words it: its [`WEB_WORDING`] replacement, or `desktop`
/// itself when the line needs none.
#[must_use]
pub fn web_wording(desktop: &str) -> &str {
    WEB_WORDING
        .iter()
        .find(|(from, _)| *from == desktop)
        .map_or(desktop, |(_, to)| to)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reworded_line_still_exists_in_the_shared_steps() {
        let mut lines: Vec<&str> = Vec::new();
        for step in STEPS {
            lines.extend(step.actions.iter().copied());
            lines.push(step.why);
            lines.push(step.intro);
            lines.push(step.check);
        }
        for (from, to) in WEB_WORDING {
            assert!(
                lines.contains(from),
                "the shared walkthrough no longer contains {from:?}: update WEB_WORDING"
            );
            assert_ne!(from, to);
            assert_eq!(web_wording(from), *to);
        }
        assert_eq!(web_wording("Click Create."), "Click Create.");
    }

    #[test]
    fn the_web_walkthrough_names_no_desktop_only_places() {
        for step in STEPS {
            for line in step.actions.iter().chain([&step.why, &step.intro]) {
                let text = web_wording(line);
                for desktop_only in [
                    "Live Render",
                    "Save Native",
                    "Chapter 8",
                    "command bar (or File",
                    "Yield Material",
                ] {
                    assert!(
                        !text.contains(desktop_only),
                        "step {:?} still says {desktop_only:?}: {text:?}",
                        step.title
                    );
                }
            }
        }
    }

    #[test]
    fn a_fresh_guide_is_closed_at_the_first_step() {
        let fresh = GuideProgress::default();
        assert!(!fresh.open && !fresh.collapsed && !fresh.float_placed);
        assert_eq!(fresh.step, 0);
        assert_eq!(GuideProgress::from_json("{}"), Ok(fresh));
    }

    #[test]
    fn progress_round_trips_through_json() {
        let progress = GuideProgress {
            open: true,
            step: 4,
            collapsed: true,
            float_placed: true,
            float_x: 512.5,
            float_y: 96.0,
        };
        let json = progress.to_json().expect("writes");
        assert_eq!(GuideProgress::from_json(&json), Ok(progress));
    }

    #[test]
    fn a_stored_step_past_the_end_or_a_bad_position_is_repaired() {
        let far = GuideProgress::from_json(r#"{"open":true,"step":999}"#).expect("parses");
        assert_eq!(far.step, STEPS.len() - 1);
        assert!(far.open);
        let lost =
            GuideProgress::from_json(r#"{"float_placed":true,"float_x":-5.0,"float_y":10.0}"#)
                .expect("parses");
        assert!(!lost.float_placed);
        assert_eq!((lost.float_x, lost.float_y), (0.0, 0.0));
        assert!(GuideProgress::from_json("not json").is_err());
        assert!(GuideProgress::from_json(r#"{"step":-1}"#).is_err());
    }
}
