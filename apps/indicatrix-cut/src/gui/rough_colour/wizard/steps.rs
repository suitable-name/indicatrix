//! The step state machine of the Rough colour wizard: which of the eight steps can be opened
//! given what the cutter has done so far. Pure data in, pure answers out; the window only
//! mirrors it.

/// The wizard's steps, in the order of the step strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Step {
    /// The capture checklist and the choice of rig.
    Checklist,
    /// The photos of every view.
    Import,
    /// The camera tier and the backlight.
    Calibration,
    /// The pixel masks.
    Masks,
    /// The polished windows and the frosted roughness.
    Surfaces,
    /// The zones and the fit.
    Fit,
    /// Photo, render and difference.
    Compare,
    /// Store the result or export the report.
    Accept,
}

impl Step {
    /// Every step, in order.
    pub const ALL: [Self; 8] = [
        Self::Checklist,
        Self::Import,
        Self::Calibration,
        Self::Masks,
        Self::Surfaces,
        Self::Fit,
        Self::Compare,
        Self::Accept,
    ];

    /// The position in [`Step::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Checklist => 0,
            Self::Import => 1,
            Self::Calibration => 2,
            Self::Masks => 3,
            Self::Surfaces => 4,
            Self::Fit => 5,
            Self::Compare => 6,
            Self::Accept => 7,
        }
    }

    /// The step at `index`, if there is one.
    #[must_use]
    pub fn from_index(index: i32) -> Option<Self> {
        usize::try_from(index)
            .ok()
            .and_then(|i| Self::ALL.get(i).copied())
    }

    /// The label of the step strip.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Checklist => "Checklist",
            Self::Import => "Photos",
            Self::Calibration => "Calibration",
            Self::Masks => "Masks",
            Self::Surfaces => "Surfaces",
            Self::Fit => "Fit",
            Self::Compare => "Compare",
            Self::Accept => "Accept",
        }
    }

    /// The step before this one.
    #[must_use]
    pub fn previous(self) -> Option<Self> {
        self.index()
            .checked_sub(1)
            .and_then(|i| Self::ALL.get(i).copied())
    }
}

/// What has been done so far; the inputs of [`reachable`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepFacts {
    /// A camera rig is picked.
    pub rig_picked: bool,
    /// The mesh is aligned to the rig (done in the locate window).
    pub aligned: bool,
    /// The number of views of the rig.
    pub views_total: usize,
    /// Views with a stone photo.
    pub stone_photos: usize,
    /// Views with a stone photo and at least one white frame.
    pub with_white_frame: usize,
    /// Views whose transmittance is computed and masked.
    pub prepared_views: usize,
    /// A camera response and a backlight spectrum are in place.
    pub calibration_applied: bool,
    /// A fit has finished and its result is shown.
    pub fit_done: bool,
    /// A background job is running.
    pub busy: bool,
}

/// The fewest views a fit needs (the solver cross-validates by leaving one view out).
pub const MIN_VIEWS: usize = 2;

/// Whether `step` can be opened now.
#[must_use]
pub fn reachable(step: Step, facts: &StepFacts) -> bool {
    blocker(step, facts).is_none()
}

/// Why `step` cannot be opened now, in a sentence for a tooltip; `None` when it can.
#[must_use]
pub fn blocker(step: Step, facts: &StepFacts) -> Option<&'static str> {
    match step {
        Step::Checklist => None,
        Step::Import => {
            if !facts.rig_picked {
                Some("Pick a camera rig first.")
            } else if !facts.aligned {
                Some("Align the mesh to the rig first (Locate inclusion from photos, step 2).")
            } else {
                None
            }
        }
        Step::Calibration => blocker(Step::Import, facts).or_else(|| {
            (facts.with_white_frame < MIN_VIEWS.min(facts.views_total.max(1)))
                .then_some("Load a stone photo and a white frame for at least two views first.")
        }),
        Step::Masks => blocker(Step::Calibration, facts).or_else(|| {
            (facts.prepared_views < MIN_VIEWS.min(facts.views_total.max(1)))
                .then_some("Prepare the photos first (Photos step).")
        }),
        Step::Surfaces => blocker(Step::Masks, facts),
        Step::Fit => blocker(Step::Surfaces, facts).or(if facts.calibration_applied {
            None
        } else {
            Some("Apply the calibration first.")
        }),
        Step::Compare | Step::Accept => blocker(Step::Fit, facts).or(if facts.fit_done {
            None
        } else {
            Some("Run the fit first.")
        }),
    }
}

/// The reachability of every step, for the step strip.
#[must_use]
pub fn reachable_all(facts: &StepFacts) -> [bool; 8] {
    let mut out = [false; 8];
    for step in Step::ALL {
        out[step.index()] = reachable(step, facts);
    }
    out
}

/// The step the Next button goes to from `current`, if it can go there.
#[must_use]
pub fn next_step(current: Step, facts: &StepFacts) -> Option<Step> {
    Step::ALL
        .get(current.index() + 1)
        .copied()
        .filter(|&next| reachable(next, facts))
}

/// The step to show when the facts change under the current one (a photo was replaced, the rig
/// changed): the current step if it is still reachable, else the last reachable step before it.
#[must_use]
pub fn settle(current: Step, facts: &StepFacts) -> Step {
    let mut step = current;
    while !reachable(step, facts) {
        match step.previous() {
            Some(previous) => step = previous,
            None => break,
        }
    }
    step
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done() -> StepFacts {
        StepFacts {
            rig_picked: true,
            aligned: true,
            views_total: 8,
            stone_photos: 8,
            with_white_frame: 8,
            prepared_views: 8,
            calibration_applied: true,
            fit_done: true,
            busy: false,
        }
    }

    #[test]
    fn nothing_done_opens_only_the_checklist() {
        let reach = reachable_all(&StepFacts::default());
        assert_eq!(
            reach,
            [true, false, false, false, false, false, false, false]
        );
    }

    #[test]
    fn a_rig_without_alignment_blocks_the_photos() {
        let facts = StepFacts {
            rig_picked: true,
            views_total: 8,
            ..StepFacts::default()
        };
        assert!(!reachable(Step::Import, &facts));
        assert!(blocker(Step::Import, &facts).unwrap().contains("Align"));
        let aligned = StepFacts {
            aligned: true,
            ..facts
        };
        assert!(reachable(Step::Import, &aligned));
        assert!(!reachable(Step::Calibration, &aligned));
    }

    #[test]
    fn calibration_needs_stone_and_white_frames_in_two_views() {
        let mut facts = StepFacts {
            rig_picked: true,
            aligned: true,
            views_total: 8,
            stone_photos: 8,
            with_white_frame: 1,
            ..StepFacts::default()
        };
        assert!(!reachable(Step::Calibration, &facts));
        facts.with_white_frame = 2;
        assert!(reachable(Step::Calibration, &facts));
    }

    #[test]
    fn masks_and_surfaces_need_prepared_views_but_not_the_calibration() {
        let mut facts = StepFacts {
            rig_picked: true,
            aligned: true,
            views_total: 8,
            stone_photos: 8,
            with_white_frame: 8,
            ..StepFacts::default()
        };
        assert!(!reachable(Step::Masks, &facts));
        facts.prepared_views = 8;
        assert!(reachable(Step::Masks, &facts));
        assert!(reachable(Step::Surfaces, &facts));
        assert!(!reachable(Step::Fit, &facts));
        facts.calibration_applied = true;
        assert!(reachable(Step::Fit, &facts));
        assert!(!reachable(Step::Compare, &facts));
        assert!(!reachable(Step::Accept, &facts));
    }

    #[test]
    fn a_finished_fit_opens_everything() {
        assert_eq!(reachable_all(&done()), [true; 8]);
        assert!(Step::ALL.iter().all(|&s| blocker(s, &done()).is_none()));
    }

    #[test]
    fn a_rig_with_two_views_needs_both() {
        let facts = StepFacts {
            rig_picked: true,
            aligned: true,
            views_total: 2,
            with_white_frame: 1,
            ..StepFacts::default()
        };
        assert!(!reachable(Step::Calibration, &facts));
    }

    #[test]
    fn next_goes_one_step_when_it_is_reachable() {
        let facts = done();
        assert_eq!(next_step(Step::Checklist, &facts), Some(Step::Import));
        assert_eq!(next_step(Step::Accept, &facts), None);
        assert_eq!(next_step(Step::Checklist, &StepFacts::default()), None);
    }

    #[test]
    fn settle_falls_back_to_the_last_reachable_step() {
        let mut facts = done();
        facts.fit_done = false;
        assert_eq!(settle(Step::Compare, &facts), Step::Fit);
        facts.calibration_applied = false;
        assert_eq!(settle(Step::Compare, &facts), Step::Surfaces);
        assert_eq!(
            settle(Step::Checklist, &StepFacts::default()),
            Step::Checklist
        );
        facts.aligned = false;
        assert_eq!(settle(Step::Fit, &facts), Step::Checklist);
    }

    #[test]
    fn indices_round_trip() {
        for (i, step) in Step::ALL.iter().enumerate() {
            assert_eq!(step.index(), i);
            assert_eq!(Step::from_index(i32::try_from(i).unwrap()), Some(*step));
        }
        assert_eq!(Step::from_index(-1), None);
        assert_eq!(Step::from_index(8), None);
        assert_eq!(Step::Checklist.previous(), None);
        assert_eq!(Step::Import.previous(), Some(Step::Checklist));
    }
}
