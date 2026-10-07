//! What a finished Optimize search reports: the candidates it offers with the numbers each was
//! ranked by, where the search started, the plain sentences about the run, and why a search
//! can fail.

use crate::retarget::{
    AnchorChange,
    metrics::RetargetMetrics,
    validity::{InvalidReason, RetargetValidity},
};
use indicatrix_cut_core::{Design, ObjectiveComponents};
use std::fmt;

/// The optical numbers a candidate was ranked by, from the search's full tilt scan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CandidateNumbers {
    /// The combined score; lower is better.
    pub score: f32,
    /// Percentage of light leaking straight out of the pavilion.
    pub windowing_pct: f32,
    /// Percentage of light returned to the eye, averaged over the tilt scan.
    pub brilliance_pct: f32,
    /// Percentage of light trapped or lost.
    pub extinction_pct: f32,
    /// `100 -` the share of the rough the finished stone keeps.
    pub yield_loss_pct: f32,
}

impl CandidateNumbers {
    pub(super) const fn new(after: ObjectiveComponents, score: f32, yield_loss_pct: f32) -> Self {
        Self {
            score,
            windowing_pct: after.windowing_pct,
            brilliance_pct: after.tilt_brilliance_pct,
            extinction_pct: after.extinction_pct,
            yield_loss_pct,
        }
    }
}

/// Where a candidate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    /// The Shift result itself (offered when it is valid).
    Shift,
    /// The part of the Shift change that is valid (offered when the whole change is not).
    Partial {
        /// How much of the Shift change, in percent.
        percent: u32,
    },
    /// A result of the search.
    Optimized,
}

/// One valid option the search offers.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchCandidate {
    /// Where it came from.
    pub kind: CandidateKind,
    /// `(tier_index, angle)` of every tier whose angle differs from the live design, leaving
    /// out the tiers that follow a relation. This is what Apply sends as the proposal.
    pub angles: Vec<(usize, f64)>,
    /// The masts that differ from the live design (re-anchored, followers included).
    pub anchors: Vec<AnchorChange>,
    /// The verdict of the validity gate (always valid for a candidate in a report).
    pub validity: RetargetValidity,
    /// The three optical columns for the metrics table.
    pub metrics: RetargetMetrics,
    /// The numbers it was ranked by.
    pub numbers: CandidateNumbers,
    /// The candidate design, with relations followed.
    pub design: Design,
}

/// Where the search started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartPoint {
    /// From the full, valid Shift result.
    Shift,
    /// From part of the Shift change, because the full one is not valid.
    Partial {
        /// How much of the Shift change, in percent.
        percent: u32,
    },
    /// From the design as it is: not even a small part of the Shift change is valid.
    Unchanged,
}

/// What a finished search found.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchReport {
    /// Where the search started.
    pub start: StartPoint,
    /// The verdict on the full Shift result (valid or not).
    pub shift_validity: RetargetValidity,
    /// The numbers of the start stone.
    pub start_numbers: CandidateNumbers,
    /// The valid options, best first (lowest score; the Shift result first on a tie).
    pub candidates: Vec<SearchCandidate>,
    /// For every result the gate refused: its reasons.
    pub dropped: Vec<Vec<InvalidReason>>,
    /// How many evaluations the search spent.
    pub evaluations: usize,
    /// How many angles the search could vary.
    pub free_tiers: usize,
    /// Whether the options were scored with the "keep the design's look" penalty.
    pub keep_look: bool,
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("{count} {one}")
    } else {
        format!("{count} {many}")
    }
}

impl SearchReport {
    /// The best option, if any.
    #[must_use]
    pub fn best(&self) -> Option<&SearchCandidate> {
        self.candidates.first()
    }

    /// How many options are results of the search (not the Shift result).
    #[must_use]
    pub fn optimized_count(&self) -> usize {
        self.candidates
            .iter()
            .filter(|candidate| candidate.kind == CandidateKind::Optimized)
            .count()
    }

    /// Plain sentences about the run: where it started, what was dropped and why, and what
    /// to try when nothing better was found.
    #[must_use]
    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        let why = self
            .shift_validity
            .reasons
            .first()
            .map(InvalidReason::message)
            .unwrap_or_default();
        match self.start {
            StartPoint::Shift => {}
            StartPoint::Partial { percent } => notes.push(format!(
                "Shift alone is not valid here. {why} The search started from {percent} % of the Shift change."
            )),
            StartPoint::Unchanged => notes.push(format!(
                "Shift alone is not valid here. {why} The search started from the design as it is."
            )),
        }
        if self.free_tiers == 0 {
            notes.push(
                "No crown or pavilion facet could be varied, so there was nothing to search."
                    .to_string(),
            );
        }
        if !self.dropped.is_empty() {
            let mut reasons: Vec<String> = Vec::new();
            for reason in self.dropped.iter().flatten() {
                let message = reason.message();
                if !reasons.contains(&message) {
                    reasons.push(message);
                }
            }
            let count = self.dropped.len();
            let counted = plural(count, "result", "results");
            let verb = if count == 1 {
                "was dropped because it is"
            } else {
                "were dropped because they are"
            };
            notes.push(format!(
                "{counted} of the search {verb} not valid: {}",
                reasons.join(" ")
            ));
        }
        if self.keep_look && self.free_tiers > 0 {
            notes.push(
                "Options are scored with a penalty for drifting from the design's table size and crown-to-pavilion ratio (keep the look)."
                    .to_string(),
            );
        }
        if self.optimized_count() == 0 && self.free_tiers > 0 {
            notes.push(match (self.candidates.is_empty(), self.start) {
                (true, _) => "The search found no valid option. Try a wider range, another objective or more steps.",
                (false, StartPoint::Shift) => "The search found nothing better than the Shift result.",
                (false, _) => "The search found nothing better than the part of the Shift change it started from.",
            }
            .to_string());
        }
        notes
    }
}

/// Why a search gave no report.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchError {
    /// The live design does not solve or close, so there is nothing to compare with.
    Unusable(InvalidReason),
    /// The search could not start (the start stone does not solve).
    Solve(String),
    /// The cancel flag was raised.
    Cancelled,
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unusable(InvalidReason::DoesNotSolve(why)) => write!(
                f,
                "Optimize needs a design that solves first. This one does not ({why})."
            ),
            Self::Unusable(InvalidReason::NotClosed) => write!(
                f,
                "Optimize needs a design whose facets enclose a stone. This one does not."
            ),
            Self::Unusable(other) => write!(f, "Optimize cannot start. {}", other.message()),
            Self::Solve(why) => write!(f, "The search could not start ({why})."),
            Self::Cancelled => write!(f, "The search was cancelled."),
        }
    }
}

impl std::error::Error for SearchError {}
