//! The two background threads of the view: the frame renderer and the scene builder.
//!
//! Both are one-slot mailboxes where the latest request wins. The UI thread only sends
//! requests and applies what comes back; a stale answer is dropped there by its
//! generation or ticket. A request that panics does not end its thread: the UI thread is
//! told, so it stops waiting for the answer, and the thread takes the next request.

use super::{
    design_mesh::MeshLibrary,
    render::{Pixels, RenderRequest, SharedPick, ViewRenderer},
    scene::{FitInputs, RoughMesh, Scene, SceneKind, StoneDraw, build_fit_scene},
};
use crate::{
    RoughPlannerWindow,
    gui::{latest_worker::LatestWorker, rough_plan::host::on_host},
};
use indicatrix_cut_core::rough_plan::RoughLayout;
use slint::Weak;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// A fit scene to build for one result.
pub(super) struct BuildRequest {
    /// Increases with every request; only the newest answer is used.
    pub(super) ticket: u64,
    /// Identifies the plan the result belongs to; design meshes are checked against the
    /// library once per plan.
    pub(super) epoch: u64,
    /// The layout to draw.
    pub(super) layout: RoughLayout,
    /// The rough the layout was planned for, with its world mesh.
    pub(super) rough: Arc<RoughMesh>,
    /// Design titles by entry id.
    pub(super) titles: BTreeMap<i64, String>,
    /// For a stone's entry id, how its design is drawn.
    pub(super) mesh_ids: BTreeMap<i64, StoneDraw>,
    /// The plan's rough colour and pose choices, when the plan has a rough colour (`zoning`
    /// builds).
    #[cfg(feature = "zoning")]
    pub(super) colour: Option<super::zoning_view::ColourJob>,
}

/// Tells the UI thread that the drawing of a result failed, so it stops waiting for it.
fn report_failure(window: &Weak<RoughPlannerWindow>) {
    let _ = window.upgrade_in_event_loop(move |_window| {
        on_host(super::draw_failed);
    });
}

/// Starts the render thread. Every finished frame is handed to the UI thread with the
/// generation of the request it answers. After a panic the renderer is rebuilt before it
/// draws again, because it may have been left in any state.
#[must_use]
pub(super) fn spawn_render_worker(
    window: Weak<RoughPlannerWindow>,
    picks: SharedPick,
) -> LatestWorker<RenderRequest> {
    let mut renderer = ViewRenderer::default();
    let rebuild = Arc::new(AtomicBool::new(false));
    let panicked = Arc::clone(&rebuild);
    let failed_window = window.clone();
    LatestWorker::spawn_with_panic_hook(
        "rough-view",
        move |request: RenderRequest| {
            if rebuild.swap(false, Ordering::AcqRel) {
                renderer = ViewRenderer::default();
            }
            let generation = request.generation;
            let pixels: Pixels = renderer.render(&request, Some(&picks));
            let _ = window.upgrade_in_event_loop(move |_window| {
                on_host(|host| super::frame_ready(host, generation, pixels));
            });
        },
        move || {
            panicked.store(true, Ordering::Release);
            report_failure(&failed_window);
        },
    )
}

/// Starts the scene builder thread, which reads design meshes through `meshes`.
#[must_use]
pub(super) fn spawn_build_worker(
    window: Weak<RoughPlannerWindow>,
    meshes: Arc<MeshLibrary>,
) -> LatestWorker<BuildRequest> {
    let failed_window = window.clone();
    LatestWorker::spawn_with_panic_hook(
        "rough-view-scene",
        move |request: BuildRequest| {
            meshes.refresh(request.epoch);
            // Zoning builds: the cutter's pose choice is applied and the stones are coloured by
            // the plan's rough colour.
            #[cfg(feature = "zoning")]
            let (posed, colours) =
                super::zoning_view::prepare(&request.layout, request.colour.as_ref(), &meshes);
            #[cfg(feature = "zoning")]
            let layout = &posed;
            #[cfg(not(feature = "zoning"))]
            let layout = &request.layout;
            let fit = build_fit_scene(
                &FitInputs {
                    layout,
                    rough: &request.rough,
                    titles: &request.titles,
                    mesh_ids: &request.mesh_ids,
                },
                &*meshes,
            );
            #[cfg(feature = "zoning")]
            let fit = {
                let mut fit = fit;
                fit.apply_stone_colours(&colours);
                fit
            };
            let scene = Arc::new(Scene::new(SceneKind::Fit(Box::new(fit))));
            let ticket = request.ticket;
            let _ = window.upgrade_in_event_loop(move |_window| {
                on_host(|host| super::scene_ready(host, ticket, scene));
            });
        },
        move || report_failure(&failed_window),
    )
}
