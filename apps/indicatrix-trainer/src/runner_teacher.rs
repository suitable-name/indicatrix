//! Which rollout samples get a finite-difference label.
//!
//! The simulator can say, at a state, how each score component moves per degree of every free
//! tier (`TeacherLabel`). A label costs one or two candidate evaluations per free tier plus the
//! oracle step, so only a share of the visited states is labelled: one in every `label_every`.
//! The choice is pure arithmetic on counters, so a run labels the same samples every time.

/// Whether the sample slot `slot` of a pass produces is labelled, for a period of `every`
/// samples (0 = never).
///
/// `base` is the number of samples collected before the pass. The sample index alone would pin
/// the labels to the same slots whenever the worker count and the period share a factor (16
/// workers, period 4: only slots 0, 4, 8 and 12 would ever be labelled, so only a quarter of the
/// corpus would ever teach). `phase` counts the finished passes and rotates the pattern by one
/// slot per pass, so every slot is labelled once every `every` passes.
pub(super) const fn wants_label(every: usize, base: usize, slot: usize, phase: usize) -> bool {
    every > 0 && (base + slot + phase).is_multiple_of(every)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_period_of_zero_labels_nothing() {
        for slot in 0..8 {
            assert!(!wants_label(0, 0, slot, 0));
            assert!(!wants_label(0, 17, slot, 5));
        }
    }

    #[test]
    fn a_period_of_one_labels_everything() {
        for (base, slot, phase) in [(0, 0, 0), (3, 2, 9), (128, 15, 7)] {
            assert!(wants_label(1, base, slot, phase));
        }
    }

    #[test]
    fn the_labels_rotate_over_the_slots_pass_by_pass() {
        // 16 slots, period 4. Without the phase only slots 0, 4, 8 and 12 would be labelled;
        // with it every slot is labelled in exactly one of any four consecutive passes, and
        // each pass labels a quarter of the slots.
        let mut labelled = [0usize; 16];
        for pass in 0..4 {
            let flagged: Vec<usize> = (0..16)
                .filter(|&w| wants_label(4, pass * 16, w, pass))
                .collect();
            assert_eq!(flagged.len(), 4, "pass {pass}");
            for w in flagged {
                labelled[w] += 1;
            }
        }
        assert_eq!(labelled, [1; 16]);
        // The period alone (phase 0, base a multiple of it) labels slot 0.
        assert!(wants_label(4, 32, 0, 0));
        assert!(!wants_label(4, 32, 1, 0));
    }

    #[test]
    fn the_share_of_labelled_samples_is_one_in_period() {
        let every = 2;
        let (workers, passes) = (6, 40);
        let mut labelled = 0;
        for pass in 0..passes {
            for slot in 0..workers {
                labelled += usize::from(wants_label(every, pass * workers, slot, pass));
            }
        }
        assert_eq!(labelled, workers * passes / every);
    }
}
