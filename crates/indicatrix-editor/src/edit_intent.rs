//! Coalesce-at-the-source UI edit intents: several raw UI events (angle-nudge
//! ticks, a slider drag's steps) merge into ONE pending intent, applied once per
//! frame rather than once per event.
//!
//! [`EditIntent::merge`] is the coalescing decision -- two nudges on the SAME
//! target set sum their angle deltas; a slider step always keeps only the newest
//! position; a handle drag keeps only the newest value. A UI keeps one single-slot
//! queue per call site, merges every posted intent into its pending one (replacing it
//! when [`EditIntent::merge`] says the two are not mergeable), and drains it every
//! [`DRAIN_INTERVAL`] (the desktop with a `slint::Timer`, the web app with
//! `requestAnimationFrame`/`setTimeout`).

use crate::manipulate::DragValue;
use std::{mem::discriminant, time::Duration};

/// How often a queue drains while an intent is pending -- one frame at 60 Hz.
pub const DRAIN_INTERVAL: Duration = Duration::from_millis(16);

/// One coalescable UI edit intent -- see the module doc comment.
#[derive(Debug, Clone, PartialEq)]
pub enum EditIntent {
    /// An angle nudge (Up/Down key or scroll wheel) on `targets` (a lone tier, or
    /// every tier in a multi-selected group -- see `setup_nudge_angle_callback`'s
    /// own doc comment for why a multi-select nudge is one `targets` set rather
    /// than one intent per tier). Repeated nudges to the SAME `targets` set within
    /// one drain window sum their `delta_deg` instead of each triggering their own
    /// apply/refresh/replan.
    NudgeAngle { targets: Vec<usize>, delta_deg: f64 },
    /// The Cut slider's tier-cutoff step (`SolidPreviewModel.tier_cutoff`). The
    /// live value is read fresh off the Slint property when the drain fires (the
    /// slider binding itself already writes it on every tick, `changed`-handler or
    /// not), so `count` only needs to distinguish "something changed" for
    /// [`EditIntent::merge`] -- carried anyway so a test can assert last-wins
    /// without a live `MainWindow`. Last-wins: only the final value posted in a
    /// drain window is ever the one a tick observes.
    CutOff { count: i32 },
    /// The Retarget dialog's crown-shift slider (`RetargetModel.crown_fraction`,
    /// read alongside `RetargetModel.scale_crown_by_ratio` at drain time -- see
    /// `setup_retarget_proposal_changed_callback`'s own doc comment). No payload:
    /// every post coalesces into the same single pending marker: last-wins.
    RetargetCrown,
    /// A handle drag of tier `tier` (`manipulate`'s angle, depth or index handle): the
    /// value the pointer currently asks for. Last-wins: pointer moves within one drain
    /// window collapse to the newest `value`, but only for the same `tier` AND the same
    /// [`DragValue`] kind -- a drag of another tier or another handle is a different
    /// gesture and never merges.
    DragTier { tier: usize, value: DragValue },
}

impl EditIntent {
    /// Merges `incoming` into `self` in place when they share the same
    /// coalescing key, returning `true` on success (`NudgeAngle`: same `targets`,
    /// deltas summed; `CutOff` and `DragTier` (same tier and value kind): newest
    /// value wins). `false` means the two are NOT
    /// mergeable (e.g. two `NudgeAngle`s naming different `targets`), and
    /// a queue instead REPLACES the pending intent outright,
    /// silently dropping whatever `self` described -- the "newer supersedes older,
    /// unread" tradeoff. In practice this replacement branch is
    /// reached only when two different targets are nudged within the same 16 ms
    /// window (a human cannot do this; only relevant to a scripted/automated
    /// input burst), so losing the superseded intent's own delta is an accepted,
    /// documented edge case rather than a silent correctness bug in ordinary use.
    pub fn merge(&mut self, incoming: &Self) -> bool {
        match (self, incoming) {
            (
                Self::NudgeAngle { targets, delta_deg },
                Self::NudgeAngle {
                    targets: other_targets,
                    delta_deg: other_delta,
                },
            ) if targets == other_targets => {
                *delta_deg += other_delta;
                true
            }
            (Self::CutOff { count }, Self::CutOff { count: other_count }) => {
                *count = *other_count;
                true
            }
            (Self::RetargetCrown, Self::RetargetCrown) => true,
            (
                Self::DragTier { tier, value },
                Self::DragTier {
                    tier: other_tier,
                    value: other_value,
                },
            ) if tier == other_tier && discriminant(value) == discriminant(other_value) => {
                *value = *other_value;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The single-slot queue's coalescing decision, without any timer: merge
    /// each post into the pending intent, or replace it -- what a real drain tick
    /// observes once it fires.
    fn coalesce(initial: EditIntent, posts: impl IntoIterator<Item = EditIntent>) -> EditIntent {
        let mut pending = initial;
        for intent in posts {
            if !pending.merge(&intent) {
                pending = intent;
            }
        }
        pending
    }

    #[test]
    fn fifty_nudges_on_the_same_tier_coalesce_to_one_summed_delta() {
        let first = EditIntent::NudgeAngle {
            targets: vec![3],
            delta_deg: 0.5,
        };
        let rest = (0..49).map(|_| EditIntent::NudgeAngle {
            targets: vec![3],
            delta_deg: 0.5,
        });
        let result = coalesce(first, rest);
        assert_eq!(
            result,
            EditIntent::NudgeAngle {
                targets: vec![3],
                delta_deg: 25.0
            },
            "50 nudges of 0.5 deg each must coalesce to one intent summing to 25.0 deg"
        );
    }

    #[test]
    fn a_nudge_burst_on_a_different_tier_replaces_rather_than_merges() {
        // Documented tradeoff (see `EditIntent::merge`'s own doc comment): a
        // different `targets` set is NOT mergeable, so the queue drops the
        // earlier, un-applied intent rather than losing track of the burst
        // entirely.
        let first = EditIntent::NudgeAngle {
            targets: vec![1],
            delta_deg: 1.0,
        };
        let result = coalesce(
            first,
            [EditIntent::NudgeAngle {
                targets: vec![2],
                delta_deg: 2.0,
            }],
        );
        assert_eq!(
            result,
            EditIntent::NudgeAngle {
                targets: vec![2],
                delta_deg: 2.0
            }
        );
    }

    #[test]
    fn a_cutoff_slider_burst_drains_to_the_last_value() {
        let first = EditIntent::CutOff { count: 10 };
        let result = coalesce(first, (11..=40).map(|count| EditIntent::CutOff { count }));
        assert_eq!(result, EditIntent::CutOff { count: 40 });
    }

    #[test]
    fn a_drag_of_the_same_tier_and_kind_keeps_the_newest_value() {
        let first = EditIntent::DragTier {
            tier: 2,
            value: DragValue::AngleDeg(40.0),
        };
        let result = coalesce(
            first,
            (1..=5).map(|step| EditIntent::DragTier {
                tier: 2,
                value: DragValue::AngleDeg(40.0 + f64::from(step)),
            }),
        );
        assert_eq!(
            result,
            EditIntent::DragTier {
                tier: 2,
                value: DragValue::AngleDeg(45.0)
            }
        );
    }

    #[test]
    fn a_drag_of_another_kind_or_tier_does_not_merge() {
        let mut pending = EditIntent::DragTier {
            tier: 2,
            value: DragValue::AngleDeg(40.0),
        };
        let other_kind = EditIntent::DragTier {
            tier: 2,
            value: DragValue::Mast(0.5),
        };
        assert!(!pending.merge(&other_kind));
        let other_tier = EditIntent::DragTier {
            tier: 3,
            value: DragValue::AngleDeg(41.0),
        };
        assert!(!pending.merge(&other_tier));
        assert_eq!(
            pending,
            EditIntent::DragTier {
                tier: 2,
                value: DragValue::AngleDeg(40.0)
            },
            "a failed merge leaves the pending intent untouched"
        );
    }

    #[test]
    fn retarget_crown_posts_always_coalesce_to_one_marker() {
        let result = coalesce(EditIntent::RetargetCrown, [EditIntent::RetargetCrown; 0]);
        assert_eq!(result, EditIntent::RetargetCrown);
        let result = coalesce(
            EditIntent::RetargetCrown,
            std::iter::repeat_n(EditIntent::RetargetCrown, 20),
        );
        assert_eq!(result, EditIntent::RetargetCrown);
    }
}
