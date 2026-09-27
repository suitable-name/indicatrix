//! Worker-thread coalescing/lifecycle tests for [`super::SolidPreviewState`],
//! plus the request-resolution unit tests split into [`replan`] (the
//! replan/plan-worker path).

use super::{
    render::render_request,
    request::RedrawRequest,
    state::{WorkerMemory, resolve_request_state},
    *,
};
use glam::Vec3;
use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

mod replan;

struct FakeSink {
    calls: Mutex<Vec<(bool, String)>>,
}

impl FakeSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<(bool, String)> {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl PreviewSink for FakeSink {
    fn apply(&self, frame: PreviewFrame) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((frame.has_solid, frame.status));
    }
}

/// Polls `f` until it stops changing for a short stability window, or panics
/// after `timeout` -- the worker thread runs asynchronously, so there is no
/// single event to block on.
fn wait_for_settled<T: PartialEq + Clone>(mut f: impl FnMut() -> T, timeout: Duration) -> T {
    let deadline = Instant::now() + timeout;
    let mut last = f();
    let mut stable_polls = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(15));
        let current = f();
        if current == last {
            stable_polls += 1;
            if stable_polls >= 4 {
                return current;
            }
        } else {
            stable_polls = 0;
            last = current;
        }
        assert!(Instant::now() < deadline, "worker thread never settled");
    }
}

fn box_planes(y_half: f32) -> Vec<(Vec3, f32)> {
    vec![
        (Vec3::X, 1.0),
        (Vec3::NEG_X, 1.0),
        (Vec3::Y, y_half),
        (Vec3::NEG_Y, y_half),
        (Vec3::Z, 1.0),
        (Vec3::NEG_Z, 1.0),
    ]
}

fn unbounded_planes() -> Vec<(Vec3, f32)> {
    vec![(Vec3::X, 1.0), (Vec3::NEG_X, 1.0)]
}

const CAMERA: CameraPose = CameraPose {
    yaw: 0.0,
    pitch: 0.0,
    distance: 5.0,
};

#[test]
fn a_single_request_eventually_reaches_the_sink() {
    let sink = FakeSink::new();
    let state = SolidPreviewState::new(sink.clone());
    state.request_redraw(box_planes(0.6), CAMERA, (16, 16), 0);

    let calls = wait_for_settled(|| sink.calls().len(), Duration::from_secs(5));
    assert_eq!(calls, 1);
    assert!(sink.calls()[0].0, "a closed box must report has_solid");
}

#[test]
fn coalesces_a_burst_of_requests_to_the_latest() {
    let sink = FakeSink::new();
    let state = SolidPreviewState::new(sink.clone());

    // Nine requests that never close, then one that does -- if coalescing works,
    // the LAST frame the sink sees must be the closed one.
    for _ in 0..9 {
        state.request_redraw(unbounded_planes(), CAMERA, (16, 16), 0);
    }
    state.request_redraw(box_planes(0.6), CAMERA, (16, 16), 0);

    let final_len = wait_for_settled(|| sink.calls().len(), Duration::from_secs(5));
    assert!(
        final_len < 10,
        "expected coalescing to avoid one render per request, got {final_len}"
    );
    let calls = sink.calls();
    let (has_solid, _status) = calls.last().expect("at least one call must have landed");
    assert!(
        *has_solid,
        "the last frame the sink receives must reflect the LAST submitted request"
    );
}

#[test]
fn a_non_closed_request_reports_has_solid_false_with_a_reason() {
    let sink = FakeSink::new();
    let state = SolidPreviewState::new(sink.clone());
    state.request_redraw(unbounded_planes(), CAMERA, (16, 16), 0);

    wait_for_settled(|| sink.calls().len(), Duration::from_secs(5));
    let calls = sink.calls();
    let (has_solid, status) = calls.last().unwrap();
    assert!(!has_solid);
    assert!(status.contains("Unbounded"), "got: {status}");
}

/// Once a real solid has closed, a temporary unbounded state must keep
/// showing the last solid, dimmed, not blank the viewport.
#[test]
fn an_unbounded_request_after_a_closed_one_keeps_showing_the_last_solid() {
    let sink = FakeSink::new();
    let state = SolidPreviewState::new(sink.clone());
    state.request_redraw(box_planes(0.6), CAMERA, (16, 16), 0);
    wait_for_settled(|| sink.calls().len(), Duration::from_secs(5));
    assert!(sink.calls().last().unwrap().0, "the first frame must close");

    state.request_redraw(unbounded_planes(), CAMERA, (16, 16), 0);
    let final_len = wait_for_settled(|| sink.calls().len(), Duration::from_secs(5));
    assert!(final_len >= 2);
    let (has_solid, status) = sink.calls().last().unwrap().clone();
    assert!(
        has_solid,
        "a held-over closed solid must still be shown, not blanked"
    );
    assert!(status.contains("Unbounded"), "got: {status}");
}

/// A facet-overlay update carries no camera/planes/`Design` of its
/// own -- it must re-render at whatever the worker already used for its last
/// `Reproject`/`Replan`, and apply to BOTH the solid and diagram styles so
/// whichever view is on screen picks it up.
#[test]
fn facet_overlay_update_reuses_the_last_context_and_applies_to_both_styles() {
    let mut memory = WorkerMemory {
        planes: Some(box_planes(0.6)),
        camera: Some(CAMERA),
        size: Some((16, 16)),
        view_mode: Some(0),
        ..WorkerMemory::default()
    };

    let overlay = FacetOverlay {
        hovered: Some(3),
        selected_facet: Some(5),
        multi_selected: vec![1, 2],
    };
    let mut mesh_cache = MeshCache::default();
    let (planes, camera, size, view_mode, style, ..) = resolve_request_state(
        &mut memory,
        &mut mesh_cache,
        RedrawRequest::UpdateFacetOverlay(overlay.clone()),
    )
    .expect(
        "memory.size is Some, so this is not the no-op case of a facet-overlay update \
         arriving before the first Planned/Reproject request",
    );

    assert_eq!(planes, box_planes(0.6), "must reuse the last-known planes");
    assert_eq!(camera, CAMERA, "must reuse the last-known camera pose");
    assert_eq!(size, (16, 16), "must reuse the last-known viewport size");
    assert_eq!(view_mode, 0, "must reuse the last-known view mode");
    assert_eq!(style.hovered, overlay.hovered);
    assert_eq!(style.selected_facet, overlay.selected_facet);
    assert_eq!(style.multi_selected, overlay.multi_selected);
    assert_eq!(
        memory.diagram.style.hovered, overlay.hovered,
        "the diagram's own style must agree, so switching to Diagram mode \
         afterward shows the same highlight"
    );
}

/// A `Reproject` request with an unchanged plane arrangement (an ordinary
/// camera drag/zoom) must leave a previously-set facet overlay in place --
/// only a genuinely different design (the plane-change check) clears it.
#[test]
fn an_ordinary_reproject_does_not_clear_a_facet_overlay() {
    let mut memory = WorkerMemory::default();
    let mut mesh_cache = MeshCache::default();
    // `memory.size` is still `None`; this is a no-op before the first frame.
    let _ = resolve_request_state(
        &mut memory,
        &mut mesh_cache,
        RedrawRequest::UpdateFacetOverlay(FacetOverlay::default()),
    );
    let _ = resolve_request_state(
        &mut memory,
        &mut mesh_cache,
        RedrawRequest::Reproject {
            planes: box_planes(0.6),
            camera: CAMERA,
            size: (16, 16),
            view_mode: 0,
            gear: None,
        },
    );
    let _ = resolve_request_state(
        &mut memory,
        &mut mesh_cache,
        RedrawRequest::UpdateFacetOverlay(FacetOverlay {
            hovered: Some(7),
            ..FacetOverlay::default()
        }),
    );

    let (_planes, _camera, _size, _view_mode, style, ..) = resolve_request_state(
        &mut memory,
        &mut mesh_cache,
        RedrawRequest::Reproject {
            planes: box_planes(0.6),
            camera: CAMERA,
            size: (16, 16),
            view_mode: 0,
            gear: None,
        },
    )
    .expect("a Reproject request always resolves");
    assert_eq!(
        style.hovered,
        Some(7),
        "an orbit/zoom reproject at the SAME planes must not drop the hover"
    );
}

/// An `UpdateFacetOverlay` arriving before this worker has ever resolved a
/// `Planned`/`Reproject` request must be a REAL no-op -- no frame at all, not
/// a background-colored 1x1 placeholder rendered and pushed to the sink as if
/// it were real. See `WorkerMemory::size`'s own doc comment.
#[test]
fn update_facet_overlay_before_any_frame_resolves_to_nothing() {
    let mut memory = WorkerMemory::default();
    let mut mesh_cache = MeshCache::default();
    assert!(
        resolve_request_state(
            &mut memory,
            &mut mesh_cache,
            RedrawRequest::UpdateFacetOverlay(FacetOverlay {
                hovered: Some(3),
                ..FacetOverlay::default()
            }),
        )
        .is_none(),
        "no Planned/Reproject frame has ever resolved, so there is nothing to redraw"
    );

    let mut rasterizer = SolidRasterizer::new(1, 1);
    let mut edges_rasterizer = SolidRasterizer::new(1, 1);
    assert!(
        render_request(
            &mut mesh_cache,
            &mut rasterizer,
            &mut edges_rasterizer,
            &mut memory,
            RedrawRequest::UpdateFacetOverlay(FacetOverlay::default()),
        )
        .is_none(),
        "render_request must not synthesize a 1x1 placeholder frame either"
    );
}
