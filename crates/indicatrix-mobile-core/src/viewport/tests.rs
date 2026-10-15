//! Viewport tests: the raster pipeline on a template design, picking, and the gestures
//! end to end (tap, orbit, pinch, handle drag, slice).

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use indicatrix_cut_core::{FreshDesignSpec, MaterialSelection, PreformSpec};
use indicatrix_editor::{
    EditorSession,
    manipulate::{DiscardReason, text},
};

use super::{
    Env, JobInputs, Outcome, Pinch, Pointer, ViewMode, ViewportState,
    frame::{self, FrameKind, RasterWorker},
    gesture::Button,
    pinch_distance,
};

const VIEW: (f32, f32) = (320.0, 200.0);

fn template_session(index: i32) -> EditorSession {
    EditorSession::from_template(
        FreshDesignSpec {
            gear_teeth: 96,
            symmetry_order: 8,
            mirror: true,
            material: MaterialSelection::none(),
            preform: PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        },
        index,
    )
}

/// A viewport, a session and a selection, the way the app holds them.
struct Rig {
    viewport: ViewportState,
    session: EditorSession,
    selected: Option<usize>,
    now: Duration,
}

impl Rig {
    fn new() -> Self {
        let mut viewport = ViewportState::default();
        viewport.set_view(VIEW.0, VIEW.1, 1.0, false);
        Self {
            viewport,
            session: template_session(1),
            selected: None,
            now: Duration::from_secs(1),
        }
    }

    fn inputs(&self) -> JobInputs {
        JobInputs {
            design: Arc::new(self.session.design.clone()),
            generation: self.session.current_generation(),
            solved: Some(self.session.design.solve().expect("the template solves")),
            selected_tier: self.selected,
            multi_selected: self.session.multi_selected.clone(),
        }
    }

    /// Plans and draws the current state on a fresh worker and lands the frame.
    fn render(&mut self) {
        let job = self.viewport.job(1, self.inputs());
        let frame = frame::render(&job).expect("a frame");
        assert_eq!(frame.kind, FrameKind::Image);
        self.viewport.land(frame);
    }

    fn step(
        &mut self,
        action: impl FnOnce(&mut ViewportState, &mut Env<'_>) -> Outcome,
    ) -> Outcome {
        let mut env = Env {
            session: &mut self.session,
            selected_tier: &mut self.selected,
            now: self.now,
        };
        action(&mut self.viewport, &mut env)
    }

    fn advance(&mut self, millis: u64) {
        self.now += Duration::from_millis(millis);
    }

    fn pointer(&mut self, event: Pointer) -> Outcome {
        self.step(|viewport, env| viewport.pointer(event, env))
    }

    fn tap(&mut self, x: f32, y: f32) -> Outcome {
        self.pointer(Pointer::Down {
            finger: 1,
            button: Button::Left,
            x,
            y,
        });
        self.advance(80);
        self.pointer(Pointer::Up { finger: 1, x, y })
    }

    /// The index of a tier on the crown with a free angle.
    fn crown_tier(&self) -> usize {
        self.session
            .design
            .tiers
            .iter()
            .position(|tier| tier.angle_deg > 20.0 && tier.angle_deg < 60.0)
            .expect("the template has a crown tier")
    }
}

#[test]
fn the_raster_size_follows_view_mode_and_caps() {
    let solid = frame::raster_size(ViewMode::Solid, false, (320.0, 200.0), 1.0, 1280);
    assert_eq!(solid, (320, 200));
    // A tall phone view is letterboxed to the minimum aspect.
    let tall = frame::raster_size(ViewMode::Solid, false, (400.0, 400.0), 1.0, 1280);
    assert_eq!(tall.0, 400);
    assert!(tall.1 < tall.0 && tall.0 as f32 / tall.1 as f32 >= frame::SOLID_MIN_ASPECT - 0.02);
    // Density is honored up to 3x and the long edge is capped.
    let dense = frame::raster_size(ViewMode::Solid, false, (800.0, 500.0), 3.0, 1280);
    assert!(dense.0.max(dense.1) <= 1280);
    let phone = frame::raster_size(ViewMode::Solid, false, (390.0, 250.0), 3.0, 1280);
    assert_eq!(phone.0, 1170, "a 3x phone draws at device pixels");
    let tablet = frame::raster_size(ViewMode::Solid, false, (800.0, 500.0), 2.0, 1600);
    assert_eq!(tablet, (1600, 1000));
    // The diagram is drawn wide enough for its labels.
    let diagram = frame::raster_size(ViewMode::Diagram, false, (320.0, 200.0), 1.0, 1280);
    assert!(diagram.0 >= frame::DIAGRAM_MIN_WIDTH);
    let enlarged = frame::raster_size(ViewMode::Diagram, true, (320.0, 200.0), 1.0, 1280);
    assert!(enlarged.0 >= frame::DIAGRAM_MIN_WIDTH_ENLARGED && enlarged.0 < diagram.0);
}

#[test]
fn a_template_renders_a_frame_of_the_requested_size() {
    let mut rig = Rig::new();
    let job = rig.viewport.job(7, rig.inputs());
    assert_eq!(job.size, (320, 200));
    let frame = frame::render(&job).expect("a frame");
    assert_eq!(frame.kind, FrameKind::Image);
    assert_eq!(frame.token, 7);
    assert_eq!(frame.size, (320, 200));
    assert_eq!(frame.rgba.len(), 320 * 200 * 4);
    assert!(frame.has_solid);
    let first = &frame.rgba[0..4];
    assert!(
        frame.rgba.chunks(4).any(|pixel| pixel != first),
        "the picture is not one flat color"
    );
    let pick = frame
        .pick
        .as_ref()
        .expect("a solid frame has a pick buffer");
    assert_eq!((pick.width, pick.height), (320, 200));
    assert!(frame.masts.is_some());
    rig.viewport.land(frame);
    assert!(
        rig.viewport
            .frame
            .as_ref()
            .is_some_and(|f| f.rgba.is_empty())
    );
}

#[test]
fn a_design_too_big_to_solve_inline_waits_for_the_solve() {
    let rig = Rig::new();
    let mut inputs = rig.inputs();
    inputs.solved = None;
    let design = Arc::clone(&inputs.design);
    let inline = indicatrix_solid::preview::view::plans_without_solver(&design)
        || indicatrix_editor::solve_policy::should_solve_synchronously_for(
            indicatrix_editor::solve_policy::SolveCostEstimate::of(&design),
            None,
        );
    let job = rig.viewport.job(1, inputs);
    let frame = frame::render(&job).expect("a frame");
    if inline {
        assert_eq!(frame.kind, FrameKind::Image);
    } else {
        // The worker does not solve a big design on its own: the last picture stays up.
        assert_eq!(frame.kind, FrameKind::Waiting);
        assert_eq!(frame.rgba.len(), 0);
        assert_ne!(frame.status.len(), 0);
    }
}

#[test]
fn a_waiting_frame_only_updates_the_status() {
    let mut rig = Rig::new();
    rig.render();
    let shown = rig.viewport.frame.as_ref().map(|frame| frame.size);
    let mut inputs = rig.inputs();
    inputs.solved = None;
    let job = rig.viewport.job(9, inputs);
    let Some(frame) = frame::render(&job) else {
        panic!("a frame");
    };
    let waiting = frame.kind == FrameKind::Waiting;
    let status = frame.status.clone();
    rig.viewport.land(frame);
    if waiting {
        assert_eq!(rig.viewport.status, status);
        assert_eq!(rig.viewport.frame.as_ref().map(|f| f.size), shown);
    }
}

#[test]
fn the_centre_pixel_of_a_top_view_hits_the_table() {
    let mut rig = Rig::new();
    rig.viewport.set_standard_view(1);
    rig.render();
    let facet = rig
        .viewport
        .facet_at(VIEW.0 / 2.0, VIEW.1 / 2.0)
        .expect("a facet at the centre");
    let tier = rig
        .viewport
        .tier_of_facet(facet)
        .expect("the facet belongs to a tier");
    assert!(
        rig.session.design.tiers[tier].angle_deg.abs() < 1e-9,
        "the table is the flat tier"
    );
}

#[test]
fn a_second_job_on_the_same_worker_only_reprojects() {
    let rig = Rig::new();
    let mut worker = RasterWorker::new();
    let mut frames = Vec::new();
    let first = rig.viewport.job(1, rig.inputs());
    worker.run(&first, &|| true, &mut |frame| frames.push(frame));
    let mut moved = ViewportState::default();
    moved.set_view(VIEW.0, VIEW.1, 1.0, false);
    moved.orbit(40.0, 10.0);
    let second = moved.job(2, rig.inputs());
    worker.run(&second, &|| true, &mut |frame| frames.push(frame));
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].generation, frames[1].generation);
    assert_ne!(frames[0].rgba, frames[1].rgba, "the camera moved");
    // Nothing changed: no frame.
    worker.run(&second, &|| true, &mut |frame| frames.push(frame));
    assert_eq!(frames.len(), 2);
}

#[test]
fn a_superseded_job_is_skipped() {
    let rig = Rig::new();
    let mut worker = RasterWorker::new();
    let mut frames = Vec::new();
    let job = rig.viewport.job(1, rig.inputs());
    worker.run(&job, &|| false, &mut |frame| frames.push(frame));
    assert!(frames.is_empty());
}

#[test]
fn the_diagram_view_renders_a_wide_picture_and_picks() {
    let mut rig = Rig::new();
    rig.viewport.set_mode(ViewMode::Diagram);
    rig.render();
    let frame = rig.viewport.frame.as_ref().expect("a frame");
    assert_eq!(frame.shown, ViewMode::Diagram);
    assert_eq!(Some(frame.size), Some(rig.viewport.raster_size()));
    assert!(frame.diagram.is_some());
    assert!(frame.pick.is_none());
    // Some pixel of the diagram lies on a facet.
    let found = (0..200).step_by(4).any(|y| {
        (0..320)
            .step_by(4)
            .any(|x| rig.viewport.facet_at(x as f32, y as f32).is_some())
    });
    assert!(found, "the diagram has facets to pick");
}

#[test]
fn tapping_a_facet_selects_its_tier_and_tapping_nothing_clears_it() {
    let mut rig = Rig::new();
    rig.viewport.set_standard_view(1);
    rig.render();
    let out = rig.tap(VIEW.0 / 2.0, VIEW.1 / 2.0);
    let Some(Some(tier)) = out.select else {
        panic!("the tap selects a tier: {out:?}");
    };
    assert_eq!(rig.selected, Some(tier));
    assert!(out.refresh && out.redraw);
    // The corner of the picture is background.
    rig.advance(800);
    let out = rig.tap(2.0, 2.0);
    assert_eq!(out.select, Some(None));
    assert_eq!(rig.selected, None);
}

#[test]
fn a_tap_clears_the_multi_selection() {
    let mut rig = Rig::new();
    rig.session.multi_selected = BTreeSet::from([0, 1]);
    rig.viewport.set_standard_view(1);
    rig.render();
    rig.tap(VIEW.0 / 2.0, VIEW.1 / 2.0);
    assert!(rig.session.multi_selected.is_empty());
}

#[test]
fn a_double_tap_resets_the_camera() {
    let mut rig = Rig::new();
    rig.render();
    rig.viewport.orbit(120.0, 40.0);
    assert_ne!(rig.viewport.camera, super::RESET_POSE);
    rig.tap(100.0, 100.0);
    rig.advance(100);
    rig.tap(101.0, 100.0);
    assert_eq!(rig.viewport.camera, super::RESET_POSE);
}

#[test]
fn a_one_finger_drag_orbits_without_selecting() {
    let mut rig = Rig::new();
    rig.render();
    let before = rig.viewport.camera;
    rig.pointer(Pointer::Down {
        finger: 1,
        button: Button::Left,
        x: 100.0,
        y: 100.0,
    });
    rig.advance(20);
    let moved = rig.pointer(Pointer::Move { x: 140.0, y: 110.0 });
    assert!(moved.redraw);
    rig.advance(20);
    let up = rig.pointer(Pointer::Up {
        finger: 1,
        x: 140.0,
        y: 110.0,
    });
    assert_eq!(up.select, None);
    assert_ne!(rig.viewport.camera.yaw, before.yaw);
    assert!(rig.viewport.camera.pitch > before.pitch);
}

#[test]
fn a_pinch_changes_the_distance_within_bounds() {
    let mut rig = Rig::new();
    rig.render();
    let before = rig.viewport.camera.distance;
    rig.step(|vp, env| vp.pinch(Pinch::Begin { x: 160.0, y: 100.0 }, env));
    rig.step(|vp, env| {
        vp.pinch(
            Pinch::Update {
                scale: 1.25,
                x: 160.0,
                y: 100.0,
            },
            env,
        )
    });
    rig.step(|vp, env| vp.pinch(Pinch::End, env));
    assert!(
        rig.viewport.camera.distance < before,
        "pinching out moves closer"
    );
    // The distance never leaves the bounds of the solid.
    let radius = rig.viewport.bounding_radius();
    let (min, max) = indicatrix_solid::preview::camera::orbit_distance_bounds(radius);
    assert_eq!(pinch_distance(before, 1000.0, radius), min);
    assert_eq!(pinch_distance(before, 0.0001, radius), max);
    assert_eq!(pinch_distance(before, f32::NAN, radius), before);
}

#[test]
fn the_wheel_zooms_and_the_right_button_pans() {
    let mut rig = Rig::new();
    rig.render();
    let before = rig.viewport.camera.distance;
    rig.step(|vp, env| vp.wheel(120.0, 160.0, 100.0, env));
    assert!(rig.viewport.camera.distance < before);
    rig.pointer(Pointer::Down {
        finger: 0,
        button: Button::Right,
        x: 150.0,
        y: 100.0,
    });
    rig.pointer(Pointer::Move { x: 170.0, y: 100.0 });
    assert!(rig.viewport.solid_view.pan_x > 0.0);
}

#[test]
fn the_diagram_zooms_between_one_and_six() {
    let mut rig = Rig::new();
    rig.viewport.set_mode(ViewMode::Diagram);
    rig.render();
    rig.step(|vp, env| vp.wheel(400.0, 160.0, 100.0, env));
    let zoomed = rig.viewport.diagram_view.zoom;
    assert!(zoomed > 1.0 && zoomed <= 6.0);
    for _ in 0..30 {
        rig.step(|vp, env| vp.wheel(400.0, 160.0, 100.0, env));
    }
    assert!((rig.viewport.diagram_view.zoom - 6.0).abs() < 1e-4);
    // A double tap restores the unzoomed layout (no panel under the finger or not).
    rig.tap(5.0, 5.0);
    rig.advance(100);
    rig.tap(5.0, 5.0);
    assert!((rig.viewport.diagram_view.zoom - 1.0).abs() < 1e-4);
}

#[test]
fn a_long_press_opens_the_text_card_of_the_facet() {
    let mut rig = Rig::new();
    rig.viewport.set_standard_view(1);
    rig.render();
    rig.pointer(Pointer::Down {
        finger: 1,
        button: Button::Left,
        x: VIEW.0 / 2.0,
        y: VIEW.1 / 2.0,
    });
    rig.advance(500);
    let out = rig.step(super::ViewportState::long_press_tick);
    assert!(out.redraw);
    let text = rig.viewport.info.clone().expect("a text card");
    assert_ne!(text.len(), 0);
    assert!(rig.viewport.close_info());
    assert!(rig.viewport.info.is_none());
}

#[test]
fn edge_presses_do_not_orbit_on_a_compact_window() {
    let mut rig = Rig::new();
    rig.viewport.set_view(VIEW.0, VIEW.1, 1.0, true);
    rig.render();
    let before = rig.viewport.camera;
    rig.pointer(Pointer::Down {
        finger: 1,
        button: Button::Left,
        x: 6.0,
        y: 100.0,
    });
    rig.advance(20);
    rig.pointer(Pointer::Move { x: 90.0, y: 100.0 });
    assert_eq!(rig.viewport.camera, before);
}

fn key_of(rig: &Rig, solved: bool) -> super::FrameKey {
    rig.viewport.frame_key(
        rig.session.current_generation(),
        solved,
        rig.selected,
        &rig.session.multi_selected,
    )
}

#[test]
fn the_frame_key_tracks_what_the_picture_depends_on() {
    let mut rig = Rig::new();
    let key = key_of(&rig, false);
    assert_eq!(key, key_of(&rig, false));
    assert_ne!(
        key,
        key_of(&rig, true),
        "a landed solve needs a new picture"
    );
    rig.viewport.orbit(10.0, 0.0);
    let orbited = key_of(&rig, false);
    assert_ne!(key, orbited);
    rig.viewport.set_mode(ViewMode::Diagram);
    assert_ne!(orbited, key_of(&rig, false));
    rig.viewport.set_view(500.0, 300.0, 1.0, false);
    let resized = key_of(&rig, false);
    rig.selected = Some(0);
    assert_ne!(resized, key_of(&rig, false));
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "test positions along a unit direction; unfused multiply-add is exact enough"
)]
fn dragging_the_angle_handle_edits_the_tier_as_one_undo_step() {
    let mut rig = Rig::new();
    let tier = rig.crown_tier();
    rig.selected = Some(tier);
    rig.render();
    let overlay = rig.viewport.handle_overlay(rig.selected);
    assert!(overlay.visible, "the selected crown tier carries handles");
    let before = rig.session.design.tiers[tier].angle_deg;

    let tip = overlay.angle;
    let (dx, dy) = (tip.0 - overlay.anchor.0, tip.1 - overlay.anchor.1);
    let length = dx.hypot(dy).max(1e-3);
    let dir = (dx / length, dy / length);

    rig.pointer(Pointer::Down {
        finger: 1,
        button: Button::Left,
        x: tip.0,
        y: tip.1,
    });
    assert!(rig.viewport.is_dragging(), "the press grabbed the handle");
    let mut live = false;
    for step in 1..=4 {
        rig.advance(20);
        let out = rig.pointer(Pointer::Move {
            x: tip.0 + dir.0 * 8.0 * step as f32,
            y: tip.1 + dir.1 * 8.0 * step as f32,
        });
        live |= out.live_edit;
    }
    assert!(live, "moves apply live edits");
    rig.advance(20);
    let end = rig.pointer(Pointer::Up {
        finger: 1,
        x: tip.0 + dir.0 * 32.0,
        y: tip.1 + dir.1 * 32.0,
    });
    assert!(end.edited && end.refresh);
    assert!(end.toast.is_some());
    assert!(!rig.viewport.is_dragging());
    let after = rig.session.design.tiers[tier].angle_deg;
    assert!((after - before).abs() > 1e-6, "the angle changed");
    // The whole gesture is one undo step.
    rig.session
        .undo()
        .expect("undo works")
        .expect("an edit to undo");
    assert!((rig.session.design.tiers[tier].angle_deg - before).abs() < 1e-9);
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "test positions along a unit direction; unfused multiply-add is exact enough"
)]
fn cancelling_a_handle_drag_restores_the_design() {
    let mut rig = Rig::new();
    let tier = rig.crown_tier();
    rig.selected = Some(tier);
    rig.render();
    let overlay = rig.viewport.handle_overlay(rig.selected);
    let before = rig.session.design.tiers[tier].angle_deg;
    let tip = overlay.angle;
    rig.pointer(Pointer::Down {
        finger: 1,
        button: Button::Left,
        x: tip.0,
        y: tip.1,
    });
    rig.advance(20);
    let (dx, dy) = (tip.0 - overlay.anchor.0, tip.1 - overlay.anchor.1);
    let length = dx.hypot(dy).max(1e-3);
    rig.pointer(Pointer::Move {
        x: tip.0 + dx / length * 30.0,
        y: tip.1 + dy / length * 30.0,
    });
    assert_ne!(rig.session.design.tiers[tier].angle_deg, before);
    let out = rig.pointer(Pointer::Cancel);
    assert!(out.edited);
    assert!((rig.session.design.tiers[tier].angle_deg - before).abs() < 1e-9);
}

#[test]
fn handles_are_hidden_for_other_tiers_and_when_switched_off() {
    let mut rig = Rig::new();
    let tier = rig.crown_tier();
    rig.selected = Some(tier);
    rig.render();
    assert!(rig.viewport.handle_overlay(Some(tier)).visible);
    assert!(!rig.viewport.handle_overlay(Some(tier + 1)).visible);
    assert!(!rig.viewport.handle_overlay(None).visible);
    rig.viewport.handles_on = false;
    assert!(!rig.viewport.handle_overlay(Some(tier)).visible);
}

fn draw_line(rig: &mut Rig) -> Outcome {
    rig.pointer(Pointer::Down {
        finger: 1,
        button: Button::Left,
        x: 60.0,
        y: 70.0,
    });
    rig.advance(20);
    rig.pointer(Pointer::Move { x: 160.0, y: 85.0 });
    rig.advance(20);
    rig.pointer(Pointer::Up {
        finger: 1,
        x: 260.0,
        y: 100.0,
    })
}

#[test]
fn a_slice_line_makes_a_provisional_tier_that_can_be_discarded() {
    let mut rig = Rig::new();
    rig.render();
    rig.step(|vp, _| vp.set_slice_mode(true));
    assert!(rig.viewport.slice.mode);
    draw_line(&mut rig);
    assert!(rig.viewport.slice.session.is_some(), "the line made a tier");
    let tiers = rig.session.design.tiers.len();

    // The provisional design is what the next job draws.
    let job = rig.viewport.job(2, rig.inputs());
    assert!(job.provisional);
    assert_eq!(job.design.tiers.len(), tiers + 1);
    let frame = frame::render(&job).expect("a provisional frame");
    assert_eq!(frame.kind, FrameKind::Image);
    assert!(frame.provisional);
    rig.viewport.land(frame);

    // Keep refuses a facet that does not touch the stone yet, or commits it as one edit.
    let kept = rig.step(super::ViewportState::slice_keep);
    if kept.edited {
        assert_eq!(rig.session.design.tiers.len(), tiers + 1);
        assert_eq!(rig.selected, Some(tiers));
        assert!(!rig.viewport.slice.mode);
        rig.session.undo().expect("undo works").expect("an edit");
        assert_eq!(rig.session.design.tiers.len(), tiers);
    } else {
        assert_eq!(rig.session.design.tiers.len(), tiers);
        assert_eq!(kept.toast.as_deref(), Some(text::SLICE_NO_DEPTH_HINT));
        let dropped = rig.step(|vp, _| vp.slice_discard(DiscardReason::User));
        assert!(rig.viewport.slice.session.is_none());
        assert!(
            dropped
                .toast
                .is_some_and(|toast| toast.starts_with("Discarded"))
        );
    }
}

#[test]
fn leaving_slice_mode_and_edits_underneath_end_the_session() {
    let mut rig = Rig::new();
    rig.render();
    rig.step(|vp, _| vp.set_slice_mode(true));
    draw_line(&mut rig);
    assert!(rig.viewport.slice.session.is_some());
    let generation = rig.session.current_generation();

    // Nothing moved: the session lives.
    let same = rig.viewport.check_expiry(generation, rig.selected);
    assert!(same.toast.is_none());
    assert!(rig.viewport.slice.session.is_some());

    // The design moved on.
    let changed = rig.viewport.check_expiry(generation + 1, rig.selected);
    assert_eq!(changed.toast.as_deref(), Some(text::SLICE_CHANGED_TOAST));
    assert!(rig.viewport.slice.session.is_none());

    // Leaving Slice mode drops a fresh session too.
    draw_line(&mut rig);
    assert!(rig.viewport.slice.session.is_some());
    let left = rig.step(|vp, _| vp.set_slice_mode(false));
    assert!(left.toast.is_some());
    assert!(rig.viewport.slice.session.is_none());
}

#[test]
fn a_too_short_slice_line_says_so() {
    let mut rig = Rig::new();
    rig.render();
    rig.step(|vp, _| vp.set_slice_mode(true));
    rig.pointer(Pointer::Down {
        finger: 1,
        button: Button::Left,
        x: 100.0,
        y: 100.0,
    });
    rig.pointer(Pointer::Up {
        finger: 1,
        x: 100.0,
        y: 100.0,
    });
    assert!(rig.viewport.slice.session.is_none());
    assert_eq!(rig.viewport.hint, text::SLICE_TOO_SHORT_HINT);
}

#[test]
fn camera_pills_reset_the_pan() {
    let mut vp = ViewportState::default();
    vp.set_view(VIEW.0, VIEW.1, 1.0, false);
    vp.solid_view = vp.solid_view.panned(30.0, 10.0);
    vp.set_standard_view(1);
    assert_eq!(vp.solid_view, super::ViewTransform::IDENTITY);
    assert!((vp.camera.pitch - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    vp.set_standard_view(6);
    assert_eq!(vp.camera, super::RESET_POSE);
}

#[test]
fn view_mode_indices_round_trip() {
    assert_eq!(
        ViewMode::from_index(ViewMode::Diagram.index()),
        ViewMode::Diagram
    );
    assert_eq!(
        ViewMode::from_index(ViewMode::Solid.index()),
        ViewMode::Solid
    );
    assert_eq!(ViewMode::from_index(7), ViewMode::Solid);
    assert_eq!(ViewMode::Solid.code(), 0);
    assert_eq!(ViewMode::Diagram.code(), 3);
    // The Render mode is the third segment and shares the Solid raster.
    assert_eq!(
        ViewMode::from_index(ViewMode::Render.index()),
        ViewMode::Render
    );
    assert_eq!(ViewMode::Render.index(), 2);
    assert_eq!(ViewMode::Render.code(), 0);
    assert_eq!(ViewMode::Render.raster_mode(), ViewMode::Solid);
    assert_eq!(ViewMode::Diagram.raster_mode(), ViewMode::Diagram);
    assert!(ViewMode::Render.is_render() && !ViewMode::Solid.is_render());
}

#[test]
fn the_render_mode_draws_the_solid_raster_size() {
    let solid = frame::raster_size(ViewMode::Solid, false, (390.0, 250.0), 3.0, 1280);
    let render = frame::raster_size(ViewMode::Render, false, (390.0, 250.0), 3.0, 1280);
    assert_eq!(solid, render);
}

#[test]
fn a_late_scale_factor_changes_the_raster_size() {
    let mut vp = ViewportState::default();
    vp.set_view(390.0, 250.0, 1.0, true);
    assert_eq!(vp.raster_size().0, 390);
    // The window reports its real scale after the first layout.
    assert!(vp.note_scale_factor(3.0));
    assert_eq!(
        vp.raster_size().0,
        1170,
        "a 3x phone draws at device pixels"
    );
    assert!(
        !vp.note_scale_factor(3.0),
        "an unchanged scale changes nothing"
    );
    // A nonsense scale reads as 1.
    assert!(vp.note_scale_factor(f32::NAN));
    assert_eq!(vp.raster_size().0, 390);
}

#[test]
fn taps_select_nothing_in_the_render_mode_and_two_fingers_do_not_pan() {
    let mut rig = Rig::new();
    rig.render();
    let tier = rig.crown_tier();
    rig.selected = Some(tier);
    rig.viewport.set_mode(ViewMode::Render);
    let outcome = rig.tap(160.0, 100.0);
    assert_eq!(outcome.select, None);
    assert_eq!(rig.selected, Some(tier), "the selection is left alone");
    let before = rig.viewport.solid_view;
    rig.viewport.pan(30.0, 20.0);
    assert_eq!(rig.viewport.solid_view, before);
    // The camera still moves.
    let camera = rig.viewport.camera;
    rig.viewport.orbit(40.0, 10.0);
    assert_ne!(rig.viewport.camera, camera);
    // No handles are drawn over the render.
    assert!(!rig.viewport.handle_overlay(rig.selected).visible);
}

#[test]
fn handle_scale_follows_the_settings() {
    let vp = ViewportState::default();
    vp.set_options(true, true, true, 1280);
    assert!((vp.options.get().handle_scale - super::LARGE_HANDLE_SCALE_COMPACT).abs() < 1e-6);
    vp.set_options(true, false, false, 960);
    assert!((vp.options.get().handle_scale - super::LARGE_HANDLE_SCALE).abs() < 1e-6);
    assert!(!vp.options.get().snap);
    assert_eq!(vp.options.get().max_edge, 960);
    vp.set_options(false, true, true, 1280);
    assert!((vp.options.get().handle_scale - 1.0).abs() < 1e-6);
}

#[test]
fn the_raster_cap_follows_the_window_class_and_battery_saver() {
    assert_eq!(frame::max_edge_for(true, false), 1280);
    assert_eq!(frame::max_edge_for(false, false), 1600);
    assert_eq!(frame::max_edge_for(true, true), 960);
    assert_eq!(frame::max_edge_for(false, true), 960);
}

#[test]
fn a_lower_raster_cap_shrinks_the_raster() {
    let mut vp = ViewportState::default();
    vp.set_view(1200.0, 800.0, 2.0, false);
    let full = vp.raster_size();
    vp.set_options(false, false, true, 640);
    let capped = vp.raster_size();
    assert!(capped.0.max(capped.1) <= 640);
    assert!(full.0.max(full.1) > capped.0.max(capped.1));
}
