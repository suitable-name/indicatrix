//! The view's UI-thread state, and the transitions of it that need no window.
//!
//! The window glue in the parent module and in `interaction` reads what a transition
//! returns and does the rest (Slint properties, requests to the worker threads). That keeps
//! the lifecycle of a result's scene and of a click waiting out its double-click testable
//! on plain data.

use super::{
    interaction::Picked,
    render::{Hover, RenderRequest, SharedPick, reset_pose},
    scene::{RoughMesh, Scene},
    thumbnails::ThumbnailWorker,
    workers::BuildRequest,
};
use crate::gui::latest_worker::LatestWorker;
use indicatrix_cut_core::rough_plan::RoughModel;
use indicatrix_solid::preview::CameraPose;
use slint::Timer;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// How long after a click that asks for a cut a second click still counts as the other
/// half of a double-click (Slint's own default interval, which its API does not expose).
/// The cut is added when this has passed without one, or at once by the double-click.
pub(super) const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);

/// How close, in logical pixels, a second click must be to the first to be a
/// double-click (the distance Slint itself uses).
const SAME_SPOT_PX: f32 = 10.0;

/// How many built result scenes are kept for selecting a result again.
const KEPT_SCENES: usize = 4;

/// The threads of the view and what the UI thread shares with them.
pub(super) struct Runtime {
    /// Draws frames.
    pub(super) renderer: LatestWorker<RenderRequest>,
    /// Builds the scene of a result.
    pub(super) builder: LatestWorker<BuildRequest>,
    /// Draws the result thumbnails.
    pub(super) thumbnails: ThumbnailWorker,
    /// The pick buffer of the frame drawn last.
    pub(super) picks: SharedPick,
}

/// A click that asks for a cut, held back until it is clear it is not half of a
/// double-click.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PendingAdd<T> {
    /// What the click asked for.
    what: T,
    /// Where it happened, in logical pixels.
    at: (f32, f32),
    /// When it happened.
    when: Instant,
    /// The model revision it saw; a cut is added only to that model.
    revision: u64,
}

impl<T> PendingAdd<T> {
    /// Whether a click at `at`, `now`, continues the gesture of this one: soon enough and
    /// on the same spot, which is what makes Slint report a double-click.
    fn continued_by(&self, at: (f32, f32), now: Instant) -> bool {
        let (dx, dy) = (at.0 - self.at.0, at.1 - self.at.1);
        now.saturating_duration_since(self.when) < DOUBLE_CLICK_INTERVAL
            && dx.mul_add(dx, dy * dy) < SAME_SPOT_PX * SAME_SPOT_PX
    }
}

/// The clicks that asked for a cut and wait out the double-click interval.
///
/// A click on the image that would add a cut does not add it at once: the second click of
/// a double-click would otherwise land on a model the first click had already changed. The
/// click is stored here and a timer of [`DOUBLE_CLICK_INTERVAL`] is started; the cut is
/// added when the timer fires and nothing has cancelled or replaced the click, or at once
/// when the double-click arrives ([`Self::due`] serves both).
#[derive(Debug)]
pub(super) struct ClickAdds<T> {
    pending: Option<PendingAdd<T>>,
}

impl<T> Default for ClickAdds<T> {
    fn default() -> Self {
        Self { pending: None }
    }
}

impl<T: Copy> ClickAdds<T> {
    /// A click at `at`, `now`, is about to be stored: takes the earlier pending click
    /// this one does not belong to, which is due now. Returns what it asked for, unless
    /// the model changed since (`revision` is the model's now). A click that continues the
    /// earlier one's gesture replaces it (the pair is a double-click).
    pub(super) fn settle(&mut self, at: (f32, f32), now: Instant, revision: u64) -> Option<T> {
        let pending = self.pending?;
        if pending.continued_by(at, now) {
            return None;
        }
        self.pending = None;
        (pending.revision == revision).then_some(pending.what)
    }

    /// Stores the click that asked for `what` at `at`, `now`, on the model of `revision`.
    pub(super) const fn click(&mut self, what: T, at: (f32, f32), now: Instant, revision: u64) {
        self.pending = Some(PendingAdd {
            what,
            at,
            when: now,
            revision,
        });
    }

    /// The interval ran out: takes the pending click. Returns what it asked for when the
    /// model is still the one it saw (`revision`) and `may_edit` says it can be edited.
    pub(super) fn due(&mut self, revision: u64, may_edit: bool) -> Option<T> {
        let pending = self.pending.take()?;
        (may_edit && pending.revision == revision).then_some(pending.what)
    }

    /// Whether a click is waiting.
    #[cfg(test)]
    #[must_use]
    pub(super) const fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// What selecting a result asks the window to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectStep {
    /// Nothing: its scene is being built already.
    Idle,
    /// Its scene is on screen: only redraw it.
    Redraw,
    /// A scene built earlier was put on screen: redraw it.
    ShowKept,
    /// The scene has to be built.
    Build,
}

/// The view's UI-thread state.
pub(in crate::gui::rough_plan) struct ViewState {
    /// The camera.
    pub(super) pose: CameraPose,
    /// The image area in logical pixels, `(0, 0)` until it is laid out.
    pub(super) view_size: (f32, f32),
    /// Physical pixels per logical pixel.
    pub(super) scale_factor: f32,
    /// The pointer button is held on the image (an orbit drag).
    pub(super) dragging: bool,
    /// Wheel ticks arrived and the quiet time has not passed yet.
    pub(super) wheeling: bool,
    /// The image area was resized and the quiet time has not passed yet.
    pub(super) resizing: bool,
    /// Where the pointer last was over the image, `None` when it is outside.
    pub(super) pointer: Option<(f32, f32)>,
    /// What the pointer is over is looked up again when the next frame is on screen (the
    /// picture under it changed).
    pub(super) rehover: bool,
    /// What the pointer is over.
    pub(super) hover: Hover,
    /// The scene of the rough as last modelled.
    pub(super) model_scene: Option<Arc<Scene>>,
    /// The model the last model scene was drawn for.
    pub(super) last_model: Option<RoughModel>,
    /// The scene shown (the model scene or a fit scene), if any.
    pub(super) scene: Option<Arc<Scene>>,
    /// Generation of the newest render request.
    pub(super) generation: u64,
    /// Generation of the frame on screen.
    pub(super) applied: u64,
    /// Ticket of the newest scene build request.
    pub(super) build_ticket: u64,
    /// The result the shown or building fit scene is for.
    pub(super) fit_result: Option<usize>,
    /// A fit scene is being built; the scene on screen is still the previous one.
    pub(super) building: bool,
    /// Identifies the batch of thumbnails being drawn.
    pub(super) thumbnail_generation: u64,
    /// Identifies the plan whose results are shown; it changes with every new result list.
    pub(super) plan_epoch: u64,
    /// The rough of the shown plan, whose world mesh the scenes and thumbnails share.
    pub(super) rough_mesh: Option<Arc<RoughMesh>>,
    /// The last built result scenes by result index, oldest first.
    pub(super) kept_scenes: Vec<(usize, Arc<Scene>)>,
    /// The clicks waiting out the double-click interval.
    pub(super) adds: ClickAdds<Picked>,
    /// Fires when the double-click interval after a click has passed.
    pub(super) add_timer: Timer,
    /// Fires when the wheel has been quiet.
    pub(super) wheel_timer: Timer,
    /// Fires when the image area has stopped changing size.
    pub(super) resize_timer: Timer,
    /// The threads, once the window exists.
    pub(super) runtime: Option<Runtime>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            pose: reset_pose(1.5),
            view_size: (0.0, 0.0),
            scale_factor: 1.0,
            dragging: false,
            wheeling: false,
            resizing: false,
            pointer: None,
            rehover: false,
            hover: Hover::None,
            model_scene: None,
            last_model: None,
            scene: None,
            generation: 0,
            applied: 0,
            build_ticket: 0,
            fit_result: None,
            building: false,
            thumbnail_generation: 0,
            plan_epoch: 0,
            rough_mesh: None,
            kept_scenes: Vec::new(),
            adds: ClickAdds::default(),
            add_timer: Timer::default(),
            wheel_timer: Timer::default(),
            resize_timer: Timer::default(),
            runtime: None,
        }
    }
}

impl ViewState {
    /// Whether the view is being moved, so frames are drawn at half size: a drag, wheel
    /// ticks that keep coming, or a window resize.
    #[must_use]
    pub(super) const fn interactive(&self) -> bool {
        self.dragging || self.wheeling || self.resizing
    }

    /// Whether the stones of the shown result scene can be picked. While a new result is
    /// being built the scene on screen is the previous one, and its stones belong to
    /// another result.
    #[must_use]
    pub(super) const fn fit_pickable(&self) -> bool {
        !self.building
    }

    /// A built scene of result `result`, if one is kept.
    fn kept_scene(&self, result: usize) -> Option<Arc<Scene>> {
        self.kept_scenes
            .iter()
            .find(|(index, _)| *index == result)
            .map(|(_, scene)| Arc::clone(scene))
    }

    /// Keeps `scene` as the built scene of `result`, dropping the oldest beyond
    /// [`KEPT_SCENES`].
    fn keep_scene(&mut self, result: usize, scene: Arc<Scene>) {
        self.kept_scenes.retain(|(index, _)| *index != result);
        self.kept_scenes.push((result, scene));
        if self.kept_scenes.len() > KEPT_SCENES {
            self.kept_scenes.remove(0);
        }
    }

    /// Result `result` was selected. A scene that is on screen or kept from before is
    /// used; otherwise the caller has to build it (see [`Self::begin_build`]).
    pub(super) fn select(&mut self, result: usize) -> SelectStep {
        if self.fit_result == Some(result) {
            return if self.building {
                SelectStep::Idle
            } else {
                SelectStep::Redraw
            };
        }
        let Some(scene) = self.kept_scene(result) else {
            return SelectStep::Build;
        };
        self.fit_result = Some(result);
        self.building = false;
        // A build of another result that is still running must not land on this one.
        self.build_ticket += 1;
        self.scene = Some(scene);
        SelectStep::ShowKept
    }

    /// Starts the build of the scene of `result`: returns the ticket its answer must carry.
    pub(super) const fn begin_build(&mut self, result: usize) -> u64 {
        self.build_ticket += 1;
        self.fit_result = Some(result);
        self.building = true;
        self.build_ticket
    }

    /// The scene answering build `ticket` arrived. Returns whether it is the newest build
    /// and was taken; an older answer is dropped.
    pub(super) fn finish_build(&mut self, ticket: u64, scene: Arc<Scene>) -> bool {
        if self.build_ticket != ticket {
            return false;
        }
        if let Some(result) = self.fit_result {
            self.keep_scene(result, Arc::clone(&scene));
        }
        self.scene = Some(scene);
        self.building = false;
        true
    }

    /// The model view is wanted: a build under way is retired. Returns the model scene.
    pub(super) fn show_model(&mut self) -> Option<Arc<Scene>> {
        self.fit_result = None;
        self.building = false;
        self.build_ticket += 1;
        self.model_scene.clone()
    }

    /// The build or the drawing of a result failed: nothing is being built any more and
    /// a late answer is dropped, so selecting the result again tries again.
    pub(super) const fn fail_build(&mut self) {
        self.building = false;
        self.build_ticket += 1;
        self.fit_result = None;
    }

    /// A new list of results replaces the shown one: what was built for the old list is
    /// forgotten.
    pub(super) fn results_replaced(&mut self) {
        self.kept_scenes.clear();
        self.fit_result = None;
        self.plan_epoch += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::rough_plan::view::scene::SceneKind;

    fn fit_scene() -> Arc<Scene> {
        Arc::new(Scene::new(SceneKind::Fit(Box::default())))
    }

    /// Two click positions `apart` pixels from each other on a horizontal line.
    fn spot(apart: f32) -> (f32, f32) {
        (100.0 + apart, 50.0)
    }

    const WINDOW: Duration = DOUBLE_CLICK_INTERVAL;

    #[test]
    fn a_click_followed_by_a_double_click_inside_the_interval_adds_its_cut_once() {
        let mut adds = ClickAdds::<u32>::default();
        let start = Instant::now();
        // Slint reports click, click, double-click for a double-click.
        assert_eq!(adds.settle(spot(0.0), start, 7), None);
        adds.click(1, spot(0.0), start, 7);
        let second = start + Duration::from_millis(120);
        assert_eq!(
            adds.settle(spot(2.0), second, 7),
            None,
            "the second half of the gesture adds nothing by itself"
        );
        adds.click(1, spot(2.0), second, 7);
        // The double-click takes the waiting click and adds it at once.
        assert_eq!(adds.due(7, true), Some(1));
        assert!(!adds.is_pending());
        assert_eq!(adds.due(7, true), None, "the timer finds nothing to add");
    }

    #[test]
    fn a_click_left_alone_past_the_interval_adds_one_cut() {
        let mut adds = ClickAdds::<u32>::default();
        let start = Instant::now();
        adds.click(5, spot(0.0), start, 3);
        assert!(adds.is_pending());
        assert_eq!(adds.due(3, true), Some(5));
        assert_eq!(adds.due(3, true), None, "it is added once");
    }

    #[test]
    fn two_clicks_600_ms_apart_on_different_targets_add_two_cuts() {
        let mut adds = ClickAdds::<u32>::default();
        let start = Instant::now();
        adds.click(1, spot(0.0), start, 0);
        // The first click's timer fires in between and the cut raises the revision.
        assert_eq!(adds.due(0, true), Some(1));
        let later = start + Duration::from_millis(600);
        assert_eq!(adds.settle(spot(200.0), later, 1), None);
        adds.click(2, spot(200.0), later, 1);
        assert_eq!(adds.due(1, true), Some(2));
    }

    #[test]
    fn a_second_click_elsewhere_settles_the_first_at_once() {
        let mut adds = ClickAdds::<u32>::default();
        let start = Instant::now();
        adds.click(1, spot(0.0), start, 4);
        // 50 ms later but 200 px away: not a double-click, so the first cut is due now.
        let soon = start + Duration::from_millis(50);
        assert_eq!(adds.settle(spot(200.0), soon, 4), Some(1));
        adds.click(2, spot(200.0), soon, 5);
        assert_eq!(adds.due(5, true), Some(2));
        // Past the interval on the same spot is a new gesture as well.
        adds.click(3, spot(0.0), start, 5);
        let late = start + WINDOW + Duration::from_millis(1);
        assert_eq!(adds.settle(spot(0.0), late, 5), Some(3));
    }

    #[test]
    fn a_click_is_dropped_when_the_model_changed_or_editing_stopped() {
        let mut adds = ClickAdds::<u32>::default();
        let start = Instant::now();
        adds.click(1, spot(0.0), start, 2);
        assert_eq!(adds.due(3, true), None, "the model changed in between");
        adds.click(1, spot(0.0), start, 2);
        assert_eq!(adds.due(2, false), None, "a plan started running");
        assert!(!adds.is_pending());
        adds.click(1, spot(0.0), start, 2);
        assert_eq!(
            adds.settle(spot(300.0), start, 9),
            None,
            "a settled click of an older model is not added"
        );
    }

    #[test]
    fn a_result_is_built_once_then_kept_and_reselected_without_a_build() {
        let mut view = ViewState::default();
        assert_eq!(view.select(0), SelectStep::Build);
        let ticket = view.begin_build(0);
        assert!(view.building && view.fit_result == Some(0));
        assert_eq!(view.select(0), SelectStep::Idle, "already being built");

        let scene = fit_scene();
        assert!(view.finish_build(ticket, Arc::clone(&scene)));
        assert!(!view.building);
        assert_eq!(view.select(0), SelectStep::Redraw);

        // Another result, then back to the first: its scene is still there.
        let second = view.begin_build(1);
        assert!(view.finish_build(second, fit_scene()));
        let before = view.build_ticket;
        assert_eq!(view.select(0), SelectStep::ShowKept);
        assert!(
            view.build_ticket > before,
            "a build under way would be retired"
        );
        assert!(Arc::ptr_eq(view.scene.as_ref().expect("a scene"), &scene));
        assert_eq!(view.fit_result, Some(0));
        assert!(!view.building);
    }

    #[test]
    fn an_older_build_answer_is_dropped() {
        let mut view = ViewState::default();
        let first = view.begin_build(0);
        let second = view.begin_build(1);
        assert!(!view.finish_build(first, fit_scene()));
        assert!(view.building && view.scene.is_none());
        assert!(view.finish_build(second, fit_scene()));
        assert!(view.scene.is_some() && !view.building);
        assert_eq!(view.kept_scenes.len(), 1);
        assert_eq!(
            view.kept_scenes[0].0, 1,
            "kept under the result it was built for"
        );
    }

    #[test]
    fn showing_the_model_retires_the_build_under_way() {
        let mut view = ViewState::default();
        let ticket = view.begin_build(2);
        let model = fit_scene();
        view.model_scene = Some(Arc::clone(&model));
        let shown = view.show_model().expect("the model scene");
        assert!(Arc::ptr_eq(&shown, &model));
        assert!(!view.building && view.fit_result.is_none());
        assert!(
            !view.finish_build(ticket, fit_scene()),
            "the late scene is dropped"
        );
        assert!(view.scene.is_none());
    }

    #[test]
    fn a_failed_build_can_be_tried_again_and_its_late_answer_is_dropped() {
        let mut view = ViewState::default();
        let ticket = view.begin_build(3);
        view.fail_build();
        assert!(!view.building && view.fit_result.is_none());
        assert!(!view.finish_build(ticket, fit_scene()));
        assert_eq!(view.select(3), SelectStep::Build);
    }

    #[test]
    fn new_results_forget_the_scenes_built_for_the_old_ones() {
        let mut view = ViewState::default();
        let ticket = view.begin_build(0);
        assert!(view.finish_build(ticket, fit_scene()));
        let epoch = view.plan_epoch;
        view.results_replaced();
        assert!(view.kept_scenes.is_empty() && view.fit_result.is_none());
        assert_eq!(view.plan_epoch, epoch + 1);
        assert_eq!(view.select(0), SelectStep::Build);
    }

    #[test]
    fn at_most_four_built_scenes_are_kept_and_the_oldest_goes_first() {
        let mut view = ViewState::default();
        for result in 0..6 {
            let ticket = view.begin_build(result);
            assert!(view.finish_build(ticket, fit_scene()));
        }
        let kept: Vec<usize> = view.kept_scenes.iter().map(|(index, _)| *index).collect();
        assert_eq!(kept, vec![2, 3, 4, 5]);
        // Building a kept result again moves it to the newest place.
        let ticket = view.begin_build(2);
        assert!(view.finish_build(ticket, fit_scene()));
        let kept: Vec<usize> = view.kept_scenes.iter().map(|(index, _)| *index).collect();
        assert_eq!(kept, vec![3, 4, 5, 2]);
    }

    #[test]
    fn the_stones_of_the_scene_on_screen_cannot_be_picked_while_another_is_built() {
        let mut view = ViewState::default();
        assert!(view.fit_pickable());
        view.begin_build(1);
        assert!(!view.fit_pickable());
        let ticket = view.build_ticket;
        view.finish_build(ticket, fit_scene());
        assert!(view.fit_pickable());
    }

    #[test]
    fn a_drag_the_wheel_or_a_resize_make_the_view_interactive() {
        assert!(!ViewState::default().interactive());
        for cause in 0..3 {
            let mut view = ViewState::default();
            match cause {
                0 => view.dragging = true,
                1 => view.wheeling = true,
                _ => view.resizing = true,
            }
            assert!(view.interactive(), "cause {cause}");
        }
    }
}
