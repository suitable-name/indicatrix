//! Publication permission is independent of optical data reuse and presentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Displayed {
    pub generation: u64,
    pub width: u32,
    pub height: u32,
}
/// Records the stamp of the beauty frame that is now on screen.
///
/// Returns `true` when the stamp differs from the previous one, i.e. when the overlay
/// shown for the previous frame must be hidden. The frame itself is **always** shown by
/// its setter: a frame whose scene generation has already moved on (the user is still
/// dragging) is displayed like any other, and the stale stamp only keeps the overlay
/// hidden until a frame of the current generation arrives (see [`may_publish`]).
pub(super) fn record_displayed(current: &mut Option<Displayed>, stamp: Displayed) -> bool {
    let changed = *current != Some(stamp);
    *current = Some(stamp);
    changed
}
/// Rechecked on the UI thread after background completion, including queued-frame changes.
///
/// `requested` is the stamp of the frame the trace was requested for: a result is only
/// published on a frame of the same size. Its generation is not compared, because an
/// unchanged optical input (an exposure or sample-target change) keeps its data across
/// generations; what matters is that the frame on screen (`displayed`) belongs to the
/// scene that is current now (`current_generation`).
pub(super) fn may_publish(
    request: u64,
    completed: u64,
    requested: Displayed,
    displayed: Option<Displayed>,
    current_generation: u64,
    enabled: bool,
) -> bool {
    enabled
        && request == completed
        && displayed.is_some_and(|d| {
            d.generation == current_generation
                && d.width == requested.width
                && d.height == requested.height
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reordered_completions_and_a_b_a_poses_cannot_revive_old_frames() {
        let a = Displayed {
            generation: 1,
            width: 640,
            height: 480,
        };
        let b = Displayed { generation: 2, ..a };
        let a_again = Displayed { generation: 3, ..a };
        assert!(!may_publish(2, 1, b, Some(b), 2, true));
        assert!(!may_publish(3, 1, a, Some(a_again), 3, true));
        assert!(!may_publish(3, 3, a_again, Some(b), 3, true));
        assert!(may_publish(3, 3, a_again, Some(a_again), 3, true));
    }
    #[test]
    fn off_hidden_or_resized_frames_reject_even_matching_data() {
        let a = Displayed {
            generation: 8,
            width: 96,
            height: 96,
        };
        assert!(!may_publish(4, 4, a, Some(a), 8, false));
        assert!(!may_publish(4, 4, a, None, 8, true));
        assert!(!may_publish(
            4,
            4,
            a,
            Some(Displayed { width: 320, ..a }),
            8,
            true
        ));
    }
    #[test]
    fn a_result_for_a_smaller_preview_frame_is_not_published_on_the_full_frame() {
        let preview = Displayed {
            generation: 6,
            width: 320,
            height: 240,
        };
        let full = Displayed {
            width: 1280,
            height: 960,
            ..preview
        };
        assert!(may_publish(5, 5, preview, Some(preview), 6, true));
        assert!(!may_publish(5, 5, preview, Some(full), 6, true));
        // A result requested for an earlier generation of the same optical input (an
        // exposure change) is still publishable on the current frame of the same size.
        let before_exposure = Displayed {
            generation: 5,
            ..preview
        };
        assert!(may_publish(5, 5, before_exposure, Some(preview), 6, true));
    }
    /// The setters (`state.rs` local frames, `update.rs` remote frames) no longer drop a
    /// beauty frame whose scene generation moved while it was in flight; they always show
    /// it and record its own stamp. Dropping them froze the viewport for the whole of an
    /// orbit, light or slider drag. The overlay's own gating must still hold: a stale
    /// stamp is recorded, hides the overlay, and never permits publication.
    #[test]
    fn a_stale_beauty_frame_is_shown_but_blocks_publication() {
        let mut shown = None;
        let current_generation = 5;
        let stale = Displayed {
            generation: 4,
            width: 640,
            height: 480,
        };
        assert!(
            record_displayed(&mut shown, stale),
            "the stale frame is recorded and clears the previous overlay"
        );
        assert_eq!(shown, Some(stale));
        // Even a result whose request, size and serial all match is refused while the
        // frame on screen is older than the scene.
        assert!(!may_publish(1, 1, stale, shown, current_generation, true));
        // The frame of the current generation arrives: the stamp moves, the overlay is
        // cleared once more, and publication is allowed again.
        let fresh = Displayed {
            generation: current_generation,
            ..stale
        };
        assert!(record_displayed(&mut shown, fresh));
        assert!(may_publish(1, 1, fresh, shown, current_generation, true));
        // The same frame shown twice does not clear the overlay again.
        assert!(!record_displayed(&mut shown, fresh));
    }
}
