//! The result thumbnails: each layout of a finished plan drawn small, in rank order, on a
//! thread of its own.
//!
//! Unlike the view's mailbox this queue must not drop work: all ten thumbnails are drawn,
//! one after the other, and each is handed to the UI thread as soon as it is ready. The only
//! thing that stops a batch is the next batch (or a cancel), noticed between two thumbnails.
//! A thumbnail that fails to draw is replaced by a neutral placeholder and the batch goes
//! on with the next one.

use super::{
    design_mesh::MeshLibrary,
    render::{Pixels, RenderOptions, RenderRequest, ViewRenderer, reset_pose},
    scene::{FitInputs, RoughMesh, Scene, SceneKind, StoneDraw, build_fit_scene},
};
use crate::{RoughPlannerWindow, gui::rough_plan::host::on_host};
use indicatrix_cut_core::rough_plan::RoughLayout;
use slint::Weak;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

/// The size of a result thumbnail in pixels.
pub(super) const THUMBNAIL_SIZE: (u32, u32) = (176, 132);

/// The color of the placeholder that stands in for a thumbnail that could not be drawn.
const PLACEHOLDER: [u8; 4] = [0x2a, 0x2e, 0x3a, 0xff];

/// One result to draw.
pub(super) struct ThumbnailItem {
    /// The layout.
    pub(super) layout: RoughLayout,
    /// For a stone's entry id, how its design is drawn.
    pub(super) mesh_ids: BTreeMap<i64, StoneDraw>,
}

/// All the thumbnails of one set of results.
pub(super) struct ThumbnailBatch {
    /// Identifies the batch; the UI ignores thumbnails of any other.
    pub(super) generation: u64,
    /// Identifies the plan; design meshes are checked against the library once per plan.
    pub(super) epoch: u64,
    /// The rough the layouts were planned for, with its world mesh.
    pub(super) rough: Arc<RoughMesh>,
    /// Design titles by entry id.
    pub(super) titles: BTreeMap<i64, String>,
    /// The results, in rank order.
    pub(super) items: Vec<ThumbnailItem>,
}

/// The thumbnail thread and the way to stop its current batch.
pub(super) struct ThumbnailWorker {
    sender: mpsc::Sender<ThumbnailBatch>,
    current: Arc<AtomicU64>,
}

impl ThumbnailWorker {
    /// Starts the thread; finished thumbnails go to the UI thread of `window`.
    #[must_use]
    pub(super) fn spawn(window: Weak<RoughPlannerWindow>, meshes: Arc<MeshLibrary>) -> Self {
        let (sender, receiver) = mpsc::channel::<ThumbnailBatch>();
        let current = Arc::new(AtomicU64::new(0));
        let watched = Arc::clone(&current);
        let spawned = std::thread::Builder::new()
            .name("rough-view-thumbnails".to_string())
            .spawn(move || {
                let mut renderer = ViewRenderer::default();
                for batch in receiver {
                    let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        draw_batch(&batch, &mut renderer, &meshes, &watched, &window);
                    }));
                    if drawn.is_err() {
                        report_failure(&window, batch.generation);
                        // The renderer may be in any state after a panic.
                        renderer = ViewRenderer::default();
                    }
                }
            });
        if let Err(error) = spawned {
            tracing::warn!("Could not start the thumbnail thread: {error}");
        }
        Self { sender, current }
    }

    /// Queues `batch`; the batch being drawn stops after its current thumbnail. Returns
    /// false when the thread is gone (it never started, or died), so nothing will be
    /// drawn and the caller must not wait for thumbnails.
    #[must_use]
    pub(super) fn start(&self, batch: ThumbnailBatch) -> bool {
        self.current.store(batch.generation, Ordering::Release);
        self.sender.send(batch).is_ok()
    }

    /// Stops the batch being drawn (after its current thumbnail); `generation` must be
    /// newer than every batch started so far.
    pub(super) fn cancel(&self, generation: u64) {
        self.current.store(generation, Ordering::Release);
    }
}

/// Tells the UI thread that drawing batch `generation` failed, so the skeletons stop.
fn report_failure(window: &Weak<RoughPlannerWindow>, generation: u64) {
    tracing::warn!("Drawing the result thumbnails failed");
    let _ = window.upgrade_in_event_loop(move |_window| {
        on_host(|host| super::thumbnails_failed(host, generation));
    });
}

/// Draws every item of `batch` while it is still the current one.
fn draw_batch(
    batch: &ThumbnailBatch,
    renderer: &mut ViewRenderer,
    meshes: &MeshLibrary,
    current: &AtomicU64,
    window: &Weak<RoughPlannerWindow>,
) {
    meshes.refresh(batch.epoch);
    let total = batch.items.len();
    for (index, item) in batch.items.iter().enumerate() {
        if current.load(Ordering::Acquire) != batch.generation {
            return;
        }
        let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            thumbnail(renderer, batch, item, meshes)
        }));
        let pixels = drawn.unwrap_or_else(|_| {
            tracing::warn!("Drawing the thumbnail of result {} failed", index + 1);
            // The renderer may be in any state after a panic.
            *renderer = ViewRenderer::default();
            placeholder()
        });
        let generation = batch.generation;
        let last = index + 1 == total;
        let _ = window.upgrade_in_event_loop(move |_window| {
            on_host(|host| super::thumbnail_ready(host, generation, index, pixels, last));
        });
    }
}

/// The neutral picture a thumbnail that could not be drawn is shown as.
fn placeholder() -> Pixels {
    let mut pixels = Pixels::new(THUMBNAIL_SIZE.0, THUMBNAIL_SIZE.1);
    for pixel in pixels.make_mut_bytes().as_chunks_mut::<4>().0 {
        pixel.copy_from_slice(&PLACEHOLDER);
    }
    pixels
}

/// The thumbnail of one result: the stones inside the glass rough, without the saw pieces.
fn thumbnail(
    renderer: &mut ViewRenderer,
    batch: &ThumbnailBatch,
    item: &ThumbnailItem,
    meshes: &MeshLibrary,
) -> Pixels {
    let fit = build_fit_scene(
        &FitInputs {
            layout: &item.layout,
            rough: &batch.rough,
            titles: &batch.titles,
            mesh_ids: &item.mesh_ids,
        },
        meshes,
    );
    let request = RenderRequest {
        generation: 0,
        scene: Arc::new(Scene::new(SceneKind::Fit(Box::new(fit)))),
        pose: reset_pose(THUMBNAIL_SIZE.0 as f32 / THUMBNAIL_SIZE.1 as f32),
        size: THUMBNAIL_SIZE,
        options: RenderOptions {
            show_saw: false,
            ..RenderOptions::default()
        },
    };
    renderer.render(&request, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::rough_plan::view::{
        design_mesh::{DesignFacet, DesignMesh, MeshMiss, MeshSource},
        scene::box_faces,
    };
    use glam::DVec3;
    use indicatrix_cut_core::rough_plan::{
        Axis, BarCut, CutOrder, CutPlan, PlacedStone, RoughBase, RoughModel, SlabCut, StonePose,
    };

    struct Cube;

    impl MeshSource for Cube {
        fn mesh(&self, _entry_id: i64) -> Result<Arc<DesignMesh>, MeshMiss> {
            Ok(Arc::new(DesignMesh {
                facets: box_faces(DVec3::splat(-0.5), DVec3::splat(0.5))
                    .iter()
                    .map(|(normal, ring)| DesignFacet::new(*normal, ring.to_vec()))
                    .collect(),
            }))
        }
    }

    #[test]
    fn a_thumbnail_that_could_not_be_drawn_is_a_flat_neutral_picture_of_the_same_size() {
        let pixels = placeholder();
        assert_eq!((pixels.width(), pixels.height()), THUMBNAIL_SIZE);
        let (chunks, rest) = pixels.as_bytes().as_chunks::<4>();
        assert_eq!(rest.len(), 0);
        assert!(chunks.iter().all(|pixel| *pixel == PLACEHOLDER));
    }

    #[test]
    fn a_thumbnail_is_176_by_132_and_shows_the_stone() {
        let stone = PlacedStone {
            entry_id: 1,
            piece_origin_mm: [0.0; 3],
            piece_size_mm: [4.0; 3],
            stone_size_mm: [3.0; 3],
            table_axis: Axis::Y,
            carat: 1.0,
            volume_mm3: 27.0,
            pose: StonePose {
                center_mm: [2.0; 3],
                axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                mm_per_unit: 3.0,
            },
        };
        let layout = RoughLayout {
            cut_order: CutOrder::Xyz,
            stones: vec![stone],
            cut_plan: CutPlan {
                slabs: vec![SlabCut {
                    thickness_mm: 4.0,
                    bars: vec![BarCut {
                        width_mm: 4.0,
                        pieces_mm: vec![4.0],
                    }],
                }],
            },
            total_carat: 1.0,
            total_volume_mm3: 27.0,
            yield_fraction: 0.4,
            exact_fit: false,
        };
        let plan_model = RoughModel::new(
            RoughBase::Block {
                x_mm: 4.0,
                y_mm: 4.0,
                z_mm: 4.0,
            },
            Vec::new(),
        );
        let titles = BTreeMap::new();
        let mesh_ids = BTreeMap::new();
        let fit = build_fit_scene(
            &FitInputs {
                layout: &layout,
                rough: &RoughMesh::new(plan_model),
                titles: &titles,
                mesh_ids: &mesh_ids,
            },
            &Cube,
        );
        let request = RenderRequest {
            generation: 0,
            scene: Arc::new(Scene::new(SceneKind::Fit(Box::new(fit)))),
            pose: reset_pose(THUMBNAIL_SIZE.0 as f32 / THUMBNAIL_SIZE.1 as f32),
            size: THUMBNAIL_SIZE,
            options: RenderOptions {
                show_saw: false,
                ..RenderOptions::default()
            },
        };
        let pixels = ViewRenderer::default().render(&request, None);
        assert_eq!((pixels.width(), pixels.height()), THUMBNAIL_SIZE);
        // The corner is background; the middle is lit stone, brighter than the background.
        let bytes = pixels.as_bytes();
        let middle = ((66 * 176) + 88) * 4;
        assert_eq!(&bytes[0..3], &[0x12, 0x14, 0x1c]);
        assert!(
            bytes[middle + 2] > 0x50,
            "stone pixel {:?}",
            &bytes[middle..middle + 3]
        );
    }
}
