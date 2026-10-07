//! Where a planned frame and its late findings meet.
//!
//! The plan worker hands its frame on first and delivers the manufacturability findings
//! afterwards ([`LateFindings`]), so the two reach the UI thread in either order: the findings
//! can beat a slow render, or follow a frame that was drawn without them. [`LateRows`] decides,
//! on the UI thread, when the tier rows are refreshed with them: never twice for one frame,
//! never for a design the editor has left, and never lost.
//!
//! # Findings belong to a generation, not to a plan
//!
//! The editor's generation moves only when the design does. A tier selection, a Cut slider
//! move and the idle replan after a stale frame all plan the SAME design again, with a fresh
//! `Arc<Design>` and the same generation, and every one of those plans draws a frame whose
//! rows start without the concave-tool findings. [`LateRows`] therefore keeps the findings it
//! has seen for the newest generation and hands them out again whenever a frame of that
//! generation lands without them; the plan's own findings, when they come, replace the copy.
//!
//! # Only a frame that pushed rows is recorded
//!
//! A landed frame pushes rows only while the editor still holds the design it was planned from
//! (that stash is taken by the first frame to land, so a second frame of the same generation,
//! queued before the first one landed, pushes nothing). [`LateRows`] is told about the frames
//! that pushed rows and no others: it records what is ON SCREEN, so a frame that left the rows
//! alone must not claim to have shown anything. [`land_committed_frame`] does the bookkeeping
//! for one frame, and also hands a kept copy of the findings to the rows' FIRST push, so a frame
//! of a generation whose findings are known builds its rows once, not twice.

use crate::gui::solid_preview::preview_state::LateFindings;
use indicatrix_cut_core::ManufacturabilityWarning;
use std::{cell::RefCell, cmp::Ordering, sync::Arc};

/// The newest committed frame whose rows were pushed.
struct Pushed {
    /// Its generation.
    generation: u64,
    /// The findings seen for `generation` (applied, or carried by a frame), kept so that a later
    /// frame of the same generation can show them again.
    findings: Option<Arc<LateFindings>>,
    /// Whether the rows on screen show the findings: the frame carried them, or they have been
    /// pushed since.
    rows_show_findings: bool,
}

/// The UI thread's record of the committed frames it has pushed rows for and of findings that
/// arrived before their frame.
#[derive(Default)]
pub(super) struct LateRows {
    pushed: Option<Pushed>,
    /// Findings whose frame has not landed yet.
    parked: Option<Arc<LateFindings>>,
}

impl LateRows {
    /// The findings a frame of `generation` that comes without its own can show in its rows
    /// right away: the copy [`Self::frame_pushed`] would hand out for it. Read BEFORE the
    /// frame pushes its rows and passed into that push, so the rows are built once. `None`
    /// when nothing is known for `generation` yet. Changes nothing.
    pub(super) fn kept(&self, generation: u64) -> Option<Arc<LateFindings>> {
        if let Some(parked) = &self.parked
            && parked.generation == generation
        {
            return Some(Arc::clone(parked));
        }
        self.pushed
            .as_ref()
            .filter(|pushed| pushed.generation == generation)
            .and_then(|pushed| pushed.findings.clone())
    }

    /// A committed frame for `generation` has just pushed its rows, `carried_findings` saying
    /// whether they were built with the findings. Only for a frame that DID push rows: a frame
    /// that left the rows alone (see [`land_committed_frame`]) must not be reported, because
    /// this records what is on screen. Returns the findings that belong to this frame and
    /// still have to be applied, if any: the ones that were parked for it, or the ones an
    /// earlier plan of the same generation delivered (this frame's rows were built without
    /// them).
    pub(super) fn frame_pushed(
        &mut self,
        generation: u64,
        carried_findings: bool,
    ) -> Option<Arc<LateFindings>> {
        let mut findings = match self.pushed.take() {
            Some(pushed) if pushed.generation == generation => pushed.findings,
            // Findings of an older design describe nothing on this frame's rows.
            _ => None,
        };
        if let Some(parked) = self.parked.take() {
            match parked.generation.cmp(&generation) {
                Ordering::Equal => findings = Some(parked),
                // Findings for a design newer than this frame: their frame is still to come.
                Ordering::Greater => self.parked = Some(parked),
                // For a plan this frame has overtaken.
                Ordering::Less => {}
            }
        }
        let due = if carried_findings {
            None
        } else {
            findings.clone()
        };
        self.pushed = Some(Pushed {
            generation,
            rows_show_findings: carried_findings || due.is_some(),
            findings,
        });
        due
    }

    /// Findings have arrived. Returns them when the rows are to be refreshed now: their frame
    /// has landed and its rows do not show them yet. Findings whose frame has not landed are
    /// parked until it does; findings of a design the editor has since moved past are dropped.
    /// A copy for the generation on screen is always kept, the newest one, even when the rows
    /// already show findings (the frame carried them, or an earlier plan of the generation
    /// delivered them): a later frame of the generation will need them.
    pub(super) fn findings_arrived(&mut self, late: LateFindings) -> Option<Arc<LateFindings>> {
        let late = Arc::new(late);
        match &mut self.pushed {
            Some(pushed) if pushed.generation == late.generation => {
                pushed.findings = Some(Arc::clone(&late));
                let due = !pushed.rows_show_findings;
                pushed.rows_show_findings = true;
                due.then_some(late)
            }
            Some(pushed) if pushed.generation > late.generation => None,
            _ => {
                self.parked = Some(late);
                None
            }
        }
    }

    /// The rows could not take `late` after all: the editor state was held by a callback that
    /// is still running. The rows are marked as lacking the findings again, so
    /// [`Self::owed`] (a retry) or the next frame of the generation pushes them. Findings of a
    /// generation that is no longer the newest frame's are dropped.
    pub(super) fn put_back(&mut self, late: &Arc<LateFindings>) {
        if let Some(pushed) = &mut self.pushed
            && pushed.generation == late.generation
        {
            pushed.findings = Some(Arc::clone(late));
            pushed.rows_show_findings = false;
        }
    }

    /// The findings the rows of the newest frame still lack, marked as pushed: what a retry
    /// after [`Self::put_back`] sends. `None` when the rows are up to date, or when a newer
    /// frame has taken over (its own findings are handled as they arrive).
    pub(super) fn owed(&mut self) -> Option<Arc<LateFindings>> {
        let pushed = self.pushed.as_mut()?;
        if pushed.rows_show_findings {
            return None;
        }
        let findings = pushed.findings.clone()?;
        pushed.rows_show_findings = true;
        Some(findings)
    }
}

/// Lands one committed frame of `generation`: `push_rows` pushes its tier rows and reports
/// whether it did. Returns the findings that still have to be applied afterwards, if any.
///
/// - `carried` is the frame's own manufacturability pass. A frame without one is given the copy
///   [`LateRows::kept`] holds for its generation (the rows then show the findings from their
///   first push, with no second one to follow); `push_rows` receives whichever applies.
/// - `push_rows` returns `false` when the frame pushed nothing (its design was already taken by
///   an earlier frame, or the editor has moved on). `late_rows` is then left alone: its record
///   of the rows on screen is still true, and the findings that arrive for them will refresh
///   them (a frame that claimed to have pushed would have made them look up to date).
///
/// `late_rows` is not borrowed while `push_rows` runs.
pub(super) fn land_committed_frame(
    late_rows: &RefCell<LateRows>,
    generation: u64,
    carried: Option<&Arc<Vec<ManufacturabilityWarning>>>,
    push_rows: impl FnOnce(Option<&[ManufacturabilityWarning]>) -> bool,
) -> Option<Arc<LateFindings>> {
    let kept = if carried.is_none() {
        late_rows.borrow().kept(generation)
    } else {
        None
    };
    let shown = carried.or_else(|| kept.as_ref().map(|late| &late.warnings));
    if !push_rows(shown.map(|findings| findings.as_slice())) {
        return None;
    }
    late_rows
        .borrow_mut()
        .frame_pushed(generation, shown.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::Design;

    /// Findings for `generation`; `copy` tells two copies of the same generation apart by the
    /// number of warnings each carries.
    fn findings_copy(generation: u64, copy: usize) -> LateFindings {
        LateFindings {
            generation,
            design: Arc::new(Design::concave_fixture()),
            masts: Vec::new(),
            warnings: Arc::new(vec![
                ManufacturabilityWarning::ToolEnclosed {
                    tier: 0,
                    placement: 0
                };
                copy
            ]),
        }
    }

    fn findings_for(generation: u64) -> LateFindings {
        findings_copy(generation, 0)
    }

    fn generations(applied: Option<Arc<LateFindings>>) -> Option<u64> {
        let findings = applied?;
        Some(findings.generation)
    }

    /// `(generation, copy)` of the findings handed out.
    fn copies(applied: Option<Arc<LateFindings>>) -> Option<(u64, usize)> {
        let findings = applied?;
        Some((findings.generation, findings.warnings.len()))
    }

    /// The usual case: the frame is drawn without the findings and they follow.
    #[test]
    fn findings_after_a_frame_without_them_refresh_the_rows() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
        assert_eq!(generations(rows.findings_arrived(findings_for(7))), Some(7));
    }

    /// The pass beat the render worker: the frame's rows already show the findings, so the
    /// follow-up must not push them a second time.
    #[test]
    fn findings_the_frame_already_carried_are_not_pushed_again() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.frame_pushed(7, true)), None);
        assert_eq!(generations(rows.findings_arrived(findings_for(7))), None);
    }

    /// The findings beat the frame to the UI thread: they wait for it and refresh the rows
    /// right after it has pushed them.
    #[test]
    fn findings_before_their_frame_wait_for_it() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.findings_arrived(findings_for(7))), None);
        assert_eq!(generations(rows.frame_pushed(7, false)), Some(7));
    }

    /// The same race, but the frame did carry the findings after all: nothing to push for it.
    /// (A LATER frame of the generation that comes without them is a different matter, see
    /// `a_same_generation_frame_without_findings_gets_the_ones_already_seen`.)
    #[test]
    fn parked_findings_are_not_pushed_for_a_frame_that_carried_them() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.findings_arrived(findings_for(7))), None);
        assert_eq!(generations(rows.frame_pushed(7, true)), None);
    }

    /// Findings of a plan the editor has already left never touch the rows.
    #[test]
    fn findings_for_a_superseded_plan_are_dropped() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.frame_pushed(8, false)), None);
        assert_eq!(generations(rows.findings_arrived(findings_for(7))), None);
        // Nor do they wait for a frame that can no longer come.
        assert_eq!(generations(rows.frame_pushed(9, false)), None);

        // Parked findings overtaken by a newer frame are forgotten too.
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.findings_arrived(findings_for(7))), None);
        assert_eq!(generations(rows.frame_pushed(8, false)), None);
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
    }

    /// Findings for a design newer than the frame that just landed stay parked for their own.
    #[test]
    fn findings_for_a_newer_plan_wait_through_an_older_frame() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.findings_arrived(findings_for(8))), None);
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
        assert_eq!(generations(rows.frame_pushed(8, false)), Some(8));
    }

    /// A second copy of findings that have been applied does nothing.
    #[test]
    fn findings_are_applied_once() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
        assert_eq!(generations(rows.findings_arrived(findings_for(7))), Some(7));
        assert_eq!(generations(rows.findings_arrived(findings_for(7))), None);
    }

    /// F3-16: a tier click, a Cut slider move or an idle replan plans the same design again.
    /// Its frame lands without the findings, and the findings of the plan before it (the
    /// generation did not change) must come back for it: a frame of a generation whose rows
    /// already showed findings is no reason to leave them off.
    #[test]
    fn a_same_generation_frame_without_findings_gets_the_ones_already_seen() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
        assert_eq!(generations(rows.findings_arrived(findings_for(7))), Some(7));
        assert_eq!(generations(rows.findings_arrived(findings_for(7))), None);
        assert_eq!(
            generations(rows.frame_pushed(7, false)),
            Some(7),
            "the second plan's rows start without the findings"
        );
        // And they are applied once for that frame, not on every later arrival.
        assert_eq!(generations(rows.findings_arrived(findings_for(7))), None);
    }

    /// F3-16, the order that used to lose them: the second plan's findings reach the UI thread
    /// BEFORE its frame. They find rows that show the first plan's findings and so change
    /// nothing, then the frame lands with rows built without them.
    #[test]
    fn the_second_plans_findings_before_its_frame_do_not_lose_the_findings() {
        let mut rows = LateRows::default();
        // Plan A: frame first, findings after.
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
        assert_eq!(
            copies(rows.findings_arrived(findings_copy(7, 1))),
            Some((7, 1))
        );
        // Plan B (same generation): its findings win the race against its frame.
        assert_eq!(copies(rows.findings_arrived(findings_copy(7, 2))), None);
        // Its frame lands without them: the newest copy is pushed.
        assert_eq!(copies(rows.frame_pushed(7, false)), Some((7, 2)));
    }

    /// F3-16, the order that already worked: the second plan's frame lands first and the first
    /// plan's findings are pushed for it; the second plan's own findings then arrive and show
    /// nothing new for that frame.
    #[test]
    fn the_second_plans_frame_before_its_findings_is_given_the_first_plans() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
        assert_eq!(
            copies(rows.findings_arrived(findings_copy(7, 1))),
            Some((7, 1))
        );
        assert_eq!(copies(rows.frame_pushed(7, false)), Some((7, 1)));
        assert_eq!(copies(rows.findings_arrived(findings_copy(7, 2))), None);
        // A third frame of the generation is given the newest copy.
        assert_eq!(copies(rows.frame_pushed(7, false)), Some((7, 2)));
    }

    /// A frame the findings were carried by is followed by a same-generation frame that was
    /// not: the follow-up copy (kept when it arrived) is pushed for it.
    #[test]
    fn findings_a_frame_carried_are_kept_for_the_next_frame_of_the_generation() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.frame_pushed(7, true)), None);
        assert_eq!(generations(rows.findings_arrived(findings_for(7))), None);
        assert_eq!(generations(rows.frame_pushed(7, false)), Some(7));
    }

    /// The findings of one design are never shown on the rows of the next.
    #[test]
    fn a_newer_generation_does_not_reuse_older_findings() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
        assert_eq!(generations(rows.findings_arrived(findings_for(7))), Some(7));
        assert_eq!(generations(rows.frame_pushed(8, false)), None);
        assert_eq!(generations(rows.owed()), None);
        assert_eq!(
            generations(rows.findings_arrived(findings_for(8))),
            Some(8),
            "the new design's own findings are still pushed"
        );
    }

    /// F4-11: findings that met a held editor state are not marked as applied. They are owed
    /// again, once, to a retry.
    #[test]
    fn findings_that_met_a_held_editor_are_retried_not_marked_applied() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
        let due = rows
            .findings_arrived(findings_for(7))
            .expect("the frame landed without them");
        // The push found the editor state held.
        rows.put_back(&due);
        assert_eq!(generations(rows.owed()), Some(7), "the retry gets them");
        assert_eq!(generations(rows.owed()), None, "and only once");
    }

    /// F4-11: put back, the findings are applied again when the same findings arrive again
    /// or when another frame of the generation lands, whichever comes first.
    #[test]
    fn put_back_findings_are_applied_by_the_next_frame_of_their_generation() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
        let due = rows
            .findings_arrived(findings_for(7))
            .expect("the frame landed without them");
        rows.put_back(&due);
        assert_eq!(generations(rows.frame_pushed(7, false)), Some(7));

        let mut rows = LateRows::default();
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
        let due = rows
            .findings_arrived(findings_for(7))
            .expect("the frame landed without them");
        rows.put_back(&due);
        assert_eq!(
            generations(rows.findings_arrived(findings_for(7))),
            Some(7),
            "a fresh copy is applied again"
        );
    }

    /// F4-11: findings put back for a generation a newer frame has since replaced are dropped.
    #[test]
    fn put_back_findings_of_a_replaced_frame_are_forgotten() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
        let due = rows
            .findings_arrived(findings_for(7))
            .expect("the frame landed without them");
        assert_eq!(generations(rows.frame_pushed(8, false)), None);
        rows.put_back(&due);
        assert_eq!(generations(rows.owed()), None);
        assert_eq!(generations(rows.frame_pushed(8, false)), None);
    }

    /// With nothing seen there is nothing owed.
    #[test]
    fn nothing_is_owed_before_any_findings_arrive() {
        let mut rows = LateRows::default();
        assert_eq!(generations(rows.owed()), None);
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
        assert_eq!(generations(rows.owed()), None);
    }

    /// F3-19: `kept` is the copy a frame of the generation would be handed, read-only.
    #[test]
    fn kept_names_the_copy_of_the_generation_and_changes_nothing() {
        let mut rows = LateRows::default();
        assert_eq!(copies(rows.kept(7)), None, "nothing seen yet");
        assert_eq!(generations(rows.frame_pushed(7, false)), None);
        assert_eq!(copies(rows.kept(7)), None, "no findings for the rows yet");
        assert_eq!(
            copies(rows.findings_arrived(findings_copy(7, 1))),
            Some((7, 1))
        );
        assert_eq!(copies(rows.kept(7)), Some((7, 1)));
        assert_eq!(copies(rows.kept(7)), Some((7, 1)), "reading is not taking");
        assert_eq!(copies(rows.kept(8)), None, "another design's rows");
        // Findings parked for a newer design are for that design only.
        assert_eq!(copies(rows.findings_arrived(findings_copy(8, 2))), None);
        assert_eq!(copies(rows.kept(7)), Some((7, 1)));
        assert_eq!(copies(rows.kept(8)), Some((8, 2)));
        assert_eq!(copies(rows.frame_pushed(8, false)), Some((8, 2)));
    }

    /// What one landed frame's row push was given, and what the landing returned.
    struct Landing {
        /// One entry per call of the push (none when it was not called): the number of
        /// findings it was given, `None` for no pass at all.
        rows_given: Vec<Option<usize>>,
        /// The findings still to be applied afterwards, as `(generation, copy)`.
        due: Option<(u64, usize)>,
    }

    /// Lands a frame of `generation` that carries findings of size `carried` (none when
    /// `None`), whose row push reports `pushes`.
    fn land(
        rows: &RefCell<LateRows>,
        generation: u64,
        carried: Option<usize>,
        pushes: bool,
    ) -> Landing {
        let carried = carried.map(|copy| findings_copy(generation, copy).warnings);
        let mut rows_given = Vec::new();
        let due = land_committed_frame(rows, generation, carried.as_ref(), |shown| {
            rows_given.push(shown.map(<[_]>::len));
            pushes
        });
        Landing {
            rows_given,
            due: copies(due),
        }
    }

    /// F3-19, the loss: a frame that carried findings but pushed no rows (its design was taken
    /// by the frame before it) must not mark the rows on screen as showing them, or the
    /// findings that follow find nothing to refresh.
    #[test]
    fn a_frame_that_pushed_no_rows_does_not_hide_the_findings_from_the_rows() {
        let rows = RefCell::new(LateRows::default());
        // The first plan's frame lands first and pushes rows without findings.
        let first = land(&rows, 7, None, true);
        assert_eq!(first.rows_given, vec![None]);
        assert_eq!(first.due, None);
        // The second plan's frame carries findings, but its design was already taken.
        let second = land(&rows, 7, Some(1), false);
        assert_eq!(second.rows_given, vec![Some(1)], "it was offered them");
        assert_eq!(second.due, None);
        // Its findings then arrive: the rows on screen still lack them, so they are due.
        assert_eq!(
            copies(rows.borrow_mut().findings_arrived(findings_copy(7, 1))),
            Some((7, 1))
        );
    }

    /// F3-19, the needless work: a frame that pushed no rows is not given the kept copy to push
    /// a second time.
    #[test]
    fn a_frame_that_pushed_no_rows_leaves_the_record_alone() {
        let rows = RefCell::new(LateRows::default());
        assert_eq!(land(&rows, 7, None, true).due, None);
        assert_eq!(
            copies(rows.borrow_mut().findings_arrived(findings_copy(7, 2))),
            Some((7, 2))
        );
        let skipped = land(&rows, 7, None, false);
        assert_eq!(skipped.due, None, "no rows were pushed, so no refresh");
        assert_eq!(copies(rows.borrow().kept(7)), Some((7, 2)), "still kept");
        assert_eq!(copies(rows.borrow_mut().owed()), None, "nothing is owed");
    }

    /// F3-19, the double build: a frame of a generation whose findings are known gets them
    /// into its FIRST push of the rows, and nothing is pushed a second time.
    #[test]
    fn a_frame_of_a_known_generation_builds_its_rows_once_with_the_findings() {
        let rows = RefCell::new(LateRows::default());
        assert_eq!(land(&rows, 7, None, true).due, None);
        assert_eq!(
            copies(rows.borrow_mut().findings_arrived(findings_copy(7, 2))),
            Some((7, 2))
        );
        for _ in 0..3 {
            // Every frame of a Cut slider drag that pushes rows.
            let frame = land(&rows, 7, None, true);
            assert_eq!(frame.rows_given, vec![Some(2)], "built with the findings");
            assert_eq!(frame.due, None, "and not rebuilt afterwards");
        }
        // The rows show the findings, so the same findings arriving again change nothing.
        assert_eq!(
            copies(rows.borrow_mut().findings_arrived(findings_copy(7, 2))),
            None
        );
    }

    /// Findings that were parked for a frame go into that frame's first push too.
    #[test]
    fn parked_findings_are_given_to_the_first_push_of_their_frame() {
        let rows = RefCell::new(LateRows::default());
        assert_eq!(
            copies(rows.borrow_mut().findings_arrived(findings_copy(7, 1))),
            None
        );
        let frame = land(&rows, 7, None, true);
        assert_eq!(frame.rows_given, vec![Some(1)]);
        assert_eq!(frame.due, None);
        assert_eq!(
            copies(rows.borrow_mut().findings_arrived(findings_copy(7, 1))),
            None,
            "the rows already show them"
        );
    }

    /// A frame's own findings are the ones its rows show, kept copy or not; and the findings of
    /// an older design never reach a frame of a newer one.
    #[test]
    fn a_frame_shows_its_own_findings_and_never_another_designs() {
        let rows = RefCell::new(LateRows::default());
        assert_eq!(land(&rows, 7, None, true).due, None);
        assert_eq!(
            copies(rows.borrow_mut().findings_arrived(findings_copy(7, 2))),
            Some((7, 2))
        );
        let own = land(&rows, 7, Some(3), true);
        assert_eq!(own.rows_given, vec![Some(3)], "the frame's own pass wins");
        assert_eq!(own.due, None);
        let newer = land(&rows, 8, None, true);
        assert_eq!(newer.rows_given, vec![None], "design 7's findings stay out");
        assert_eq!(newer.due, None);
    }

    /// The record is never borrowed while the rows are being pushed, so a push that reaches
    /// back into it cannot panic.
    #[test]
    fn the_record_is_free_while_the_rows_are_pushed() {
        let rows = RefCell::new(LateRows::default());
        let due = land_committed_frame(&rows, 7, None, |_| {
            assert!(rows.try_borrow_mut().is_ok());
            true
        });
        assert!(due.is_none());
    }
}
